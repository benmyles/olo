// In every frame: tells the extension when a text field gets the focus
// (so the context menu holds this page's logins, and a card form's field
// Passwords' cards), fills a card it's given into the frame's card
// fields, says which of a
// login's usernames the page shows (an account chooser, "signed in as"),
// fills what Passwords gives as typing does, carries a sign-in split over
// steps on to its password and one-time code fields as they come, reports
// a login sent with a form (to offer saving it), and keeps which iframe
// the frame is (for tree.js).

(() => {
  if (window.__oloPasswords) return;
  window.__oloPasswords = true;

  // Which iframe of its parent this frame is, for the widget tree
  // (tree.js tells each iframe a token of its own).
  if (window !== window.top) {
    addEventListener("message", (e) => {
      if (e.source === window.parent && typeof e.data?.__oloFrame === "string") window.__oloFrame = e.data.__oloFrame;
    });
  }

  const TEXT = "input:not([type]),input[type=text],input[type=email],input[type=tel],input[type=number]";

  // Shown and editable: not the hidden decoys some sign-ins keep (tiny,
  // off the page, transparent or hidden from assistive technology).
  function usable(e) {
    const r = e.getBoundingClientRect();
    const st = getComputedStyle(e);
    return (
      r.width >= 4 && r.height >= 4 && r.right > 0 && r.bottom > 0 &&
      st.visibility !== "hidden" && st.display !== "none" && st.opacity !== "0" &&
      e.getAttribute("aria-hidden") !== "true" && !e.disabled && !e.readOnly
    );
  }

  // To the extension (which, after an update, may be gone from this copy).
  function tell(m) {
    try {
      return chrome.runtime.sendMessage(m).catch(() => null);
    } catch {
      return Promise.resolve(null);
    }
  }

  // Every element matching `sel` in `root` and the open shadow roots in
  // it (web components keep sign-in fields there).
  function deep(root, sel, out = []) {
    out.push(...root.querySelectorAll(sel));
    const walk = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT);
    for (let n = walk.currentNode; n; n = walk.nextNode()) if (n.shadowRoot) deep(n.shadowRoot, sel, out);
    return out;
  }

  const all = (sel) => deep(document, sel).filter(usable);

  // The active element, inside shadow roots too.
  function active() {
    let a = document.activeElement;
    while (a && a.shadowRoot && a.shadowRoot.activeElement) a = a.shadowRoot.activeElement;
    return a;
  }

  function editable(e) {
    return e && (e instanceof HTMLInputElement || e instanceof HTMLTextAreaElement || e.isContentEditable);
  }

  // What a field says it is for: its autocomplete, name, id, label,
  // placeholder and accessible name.
  function says(e) {
    const label = e.labels && e.labels[0] ? e.labels[0].textContent : "";
    const by = (e.getAttribute("aria-labelledby") || "").split(/\s+/).map((id) => document.getElementById(id)?.textContent || "").join(" ");
    return [e.getAttribute("autocomplete"), e.name, e.id, label, by, e.placeholder, e.getAttribute("aria-label")].join(" ").toLowerCase();
  }

  function isOtp(e) {
    if (!(e instanceof HTMLInputElement) || e.type === "password" && !/one-time-code/.test(e.getAttribute("autocomplete") || "")) return false;
    if (/one-time-code/.test(e.getAttribute("autocomplete") || "")) return true;
    // (Not "security code": a card's is that.)
    return /\b(otp|totp|2fa|mfa|one.?time|verification|auth(entication|enticator)?)\b.*\bcode\b|\b(otp|totp|2fa|mfa|passcode)\b/.test(says(e));
  }

  // The password field to fill: the focused one, else the current
  // password (not a new one being set), else the first.
  function passwordField() {
    const a = active();
    if (a && a.type === "password" && usable(a) && !isOtp(a)) return a;
    const pw = all("input[type=password]").filter((e) => !isOtp(e));
    return pw.find((e) => /current-password/.test(e.getAttribute("autocomplete") || "")) || pw.find((e) => !/new-password/.test(e.getAttribute("autocomplete") || "")) || pw[0] || null;
  }

  // The username field for `pass`: the text field just before it (in its
  // form, else the page); with no password field (sign-ins that ask for
  // the account first), the focused text field, else one that says it is
  // for the account, else an email field, else the only one.
  function userField(pass) {
    const texts = all(TEXT).filter((e) => !isOtp(e));
    const named = (e) => /username|email|e-mail|login|account|ident|user|phone/.test(says(e));
    if (pass) {
      const scope = pass.form ? texts.filter((t) => t.form === pass.form) : texts;
      const before = scope.filter((t) => t.compareDocumentPosition(pass) & Node.DOCUMENT_POSITION_FOLLOWING);
      return before.reverse().find(named) || before[0] || null;
    }
    const a = active();
    return (
      (a && texts.includes(a) ? a : null) ||
      texts.find((e) => /username|email/.test(e.getAttribute("autocomplete") || "")) ||
      texts.find(named) ||
      texts.find((e) => e.type === "email") ||
      (texts.length === 1 ? texts[0] : null)
    );
  }

  function otpField() {
    const a = active();
    if (a && isOtp(a) && usable(a)) return a;
    return all("input").find(isOtp) || null;
  }

  // Set a field's value as typing would: focused, each frameworks'
  // events (React and the like listen for input; some for keys).
  function set(e, v) {
    e.focus();
    if (e.isContentEditable) {
      document.execCommand("selectAll", false);
      document.execCommand("insertText", false, v);
      return;
    }
    const proto = e instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    e.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: v.slice(-1) || "" }));
    try {
      Object.getOwnPropertyDescriptor(proto, "value").set.call(e, v);
    } catch {
      e.value = v;
    }
    e.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertText", data: v }));
    e.dispatchEvent(new KeyboardEvent("keyup", { bubbles: true, key: v.slice(-1) || "" }));
    e.dispatchEvent(new Event("change", { bubbles: true }));
  }

  // A one-time code into a field each digit (some sites split it).
  function setOtp(field, code) {
    const group = field.maxLength === 1 ? all("input").filter((e) => e.maxLength === 1 && e.form === field.form) : [];
    if (group.length >= code.length) group.slice(0, code.length).forEach((e, i) => set(e, code[i]));
    else set(field, code);
  }

  // The login, as far as this step of the sign-in asks: the one-time code
  // if that is what has the focus, else the username and password (what
  // fields there are). Says what it filled.
  function fill({ username, password, otp }) {
    const a = active();
    if (otp && a && isOtp(a)) {
      setOtp(a, otp);
      return { otp: true };
    }
    const pass = passwordField();
    const user = userField(pass);
    let did = {};
    if (user && username) {
      set(user, username);
      did.username = true;
    }
    if (pass && password) {
      set(pass, password);
      did.password = true;
    }
    if (!did.username && !did.password && otp) {
      const o = otpField();
      if (o) {
        setOtp(o, otp);
        return { otp: true };
      }
    }
    (pass || user)?.focus();
    return did;
  }

  // ---- Credit cards ----

  // What a field of a card form takes: its autocomplete token (cc-number,
  // cc-name, cc-given-name, cc-family-name, cc-exp, cc-exp-month,
  // cc-exp-year, cc-csc), else what it says (a card form that doesn't).
  function cardKind(e) {
    if (!(e instanceof HTMLInputElement || e instanceof HTMLSelectElement)) return null;
    if (e instanceof HTMLInputElement && !/^(text|tel|number|search|password)$/.test(e.type || "text")) return null;
    const ac = (e.getAttribute("autocomplete") || "").toLowerCase();
    const token = ac.match(/\bcc-(number|name|given-name|family-name|exp-month|exp-year|exp|csc)\b/);
    if (token) return token[1];
    if (/\b(username|email|current-password|new-password|one-time-code|postal-code|tel)\b/.test(ac)) return null;
    const s = says(e);
    if (/card.?(number|num\b|no\b)|\bccnum|cc-?number|cardnumber|número de (la )?tarjeta|kartennummer|numéro de (la )?carte/.test(s)) return "number";
    if (/\b(cvc|cvv2?|csc|cvn|cid)\b|security.?code|card.?code|card.?verification/.test(s)) return "csc";
    if (/(exp|expir|valid).{0,20}(month|\bmm\b)|cc-?month|exp-?month/.test(s)) return "exp-month";
    if (/(exp|expir|valid).{0,20}(year|\byy)|cc-?year|exp-?year/.test(s)) return "exp-year";
    if (/expir|exp.?date|valid.?(thru|through|until)|\bmm\s*\/\s*yy/.test(s)) return "exp";
    if (/name.?on.?card|card.?holder|holder.?name|nameoncard|cc-?name/.test(s)) return "name";
    return null;
  }

  // The card fields beside `e` (in its form, else the frame), by kind.
  function cardFields(e) {
    const scope = e && e.form ? e.form : document;
    const kinds = new Map();
    for (const f of deep(scope, "input,select").filter(usable)) {
      const k = cardKind(f);
      if (k) kinds.set(k, [...(kinds.get(k) || []), f]);
    }
    return kinds;
  }

  // Choose the option of `sel` that is one of `wanted` (by value or text,
  // a month as "08", "8", "Aug" or "August", ignoring case).
  function choose(sel, wanted) {
    const norm = (v) => (v || "").trim().toLowerCase();
    const want = wanted.map(norm);
    const opt = [...sel.options].find((o) => want.includes(norm(o.value)) || want.includes(norm(o.text)) || want.some((w) => w.length > 2 && norm(o.text).startsWith(w)));
    if (!opt) return false;
    sel.focus();
    sel.value = opt.value;
    sel.dispatchEvent(new Event("input", { bubbles: true }));
    sel.dispatchEvent(new Event("change", { bubbles: true }));
    return true;
  }

  // A card into the frame's card fields, as typing would: its number,
  // the name (split where the form asks for given and family names), the
  // expiry in the form's shape (one field, "MM/YY" or as long as it takes,
  // or month and year, typed or chosen) and the security code. Says what
  // it filled.
  function fillCard(c) {
    // (The field the menu was opened on, not what has the focus now:
    // another frame's fill may have taken it.)
    const anchor = lastOver && lastOver.isConnected && cardKind(lastOver) ? lastOver : active();
    const kinds = cardFields(anchor);
    const did = [];
    const months = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];
    const m = parseInt(c.expiry_month, 10);
    const mm = c.expiry_month ? String(m).padStart(2, "0") : "";
    const yyyy = c.expiry_year || "";
    const yy = yyyy.slice(-2);
    const put = (kind, value, choices) => {
      if (!value) return;
      for (const f of kinds.get(kind) || []) {
        if (f instanceof HTMLSelectElement) {
          if (choose(f, choices || [value])) did.push(kind);
        } else {
          set(f, value);
          did.push(kind);
        }
      }
    };
    put("number", c.number);
    put("name", c.cardholder);
    const [given, ...rest] = (c.cardholder || "").trim().split(/\s+/);
    put("given-name", given);
    put("family-name", rest.join(" "));
    if (mm && yyyy) {
      for (const f of kinds.get("exp") || []) {
        // Its shape as it says it ("MM / YY", "MM/YYYY"), else as long
        // as it takes (4: MMYY, 6: MMYYYY), else MM/YY, the usual.
        const said = [f.placeholder, f.getAttribute("aria-label"), f.title].join(" ").toLowerCase();
        const sep = /mm\s+\/\s+[ya]/.test(said) ? " / " : "/";
        const value = /(yyyy|aaaa|jjjj)/.test(said) ? `${mm}${sep}${yyyy}` : /\b(yy|aa|jj)\b/.test(said) ? `${mm}${sep}${yy}` : f.maxLength === 4 ? mm + yy : f.maxLength === 6 ? mm + yyyy : `${mm}/${yy}`;
        set(f, value);
        did.push("exp");
      }
      put("exp-month", mm, [mm, String(m), months[m - 1], months[m - 1].slice(0, 3)]);
      put("exp-year", yyyy, [yyyy, yy]);
    }
    put("csc", c.code);
    // (The number's field keeps the focus, as a person's fill would.)
    (kinds.get("number") || [])[0]?.focus();
    return { filled: did };
  }

  // Which of `names` (a login's usernames) the page shows: in its text,
  // or in a field's value (the account a two-step sign-in asked for).
  function seen(names) {
    const text = ((document.body && document.body.innerText) || "").slice(0, 200000).toLowerCase();
    const values = all("input").map((e) => (e.value || "").toLowerCase()).join("\n");
    return names.filter((n) => {
      const k = (n || "").toLowerCase();
      return k.length >= 3 && (text.includes(k) || values.includes(k));
    });
  }

  chrome.runtime.onMessage.addListener((m, sender, reply) => {
    if (m.origin && m.origin !== location.origin) return;
    if (m.type === "seen") {
      reply({ seen: seen(m.names || []) });
    } else if (m.type === "set") {
      const e = active();
      if (editable(e) && !e.disabled && !e.readOnly) set(e, m.value);
    } else if (m.type === "fill") {
      reply(fill(m));
    } else if (m.type === "fill-card") {
      reply(fillCard(m));
    } else if (m.type === "next-step") {
      // A sign-in carried on from its first step: what comes now.
      reply(nextStep(m));
    } else if (m.type === "watch") {
      watch();
    }
  });

  // A password or one-time code field shown since the sign-in started
  // here (it asked for the account first): filled from the login chosen.
  function nextStep(m) {
    const pass = passwordField();
    if (pass && m.password && !pass.value) {
      set(pass, m.password);
      pass.focus();
      return { password: true };
    }
    const o = otpField();
    if (o && m.otp && !o.value) {
      setOtp(o, m.otp);
      return { otp: true };
    }
    return {};
  }

  // While a sign-in is under way: tell the extension when a password or
  // code field turns up (a page that shows the next step in place).
  let watching = null;
  function watch() {
    if (watching) return;
    let seenBefore = !!(passwordField() || otpField());
    const check = () => {
      const now = !!(passwordField() || otpField());
      if (now && !seenBefore) tell({ type: "step" });
      seenBefore = now;
    };
    let timer = 0;
    watching = new MutationObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(check, 250);
    });
    watching.observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ["type", "style", "class", "hidden"] });
    setTimeout(() => {
      watching?.disconnect();
      watching = null;
    }, 3 * 60 * 1000);
  }

  // The context menu follows the field: made again when one gets the
  // focus or the pointer comes over it (Chrome shows the menu as it is
  // when the button goes down, so it must be ready before; the page may
  // show another account by now).
  let lastOver = null, lastAt = 0;
  const focused = (e) => {
    const t = e.composedPath ? e.composedPath()[0] : e.target;
    if (!editable(t)) return;
    if (e.type === "pointerover" && t === lastOver && Date.now() - lastAt < 2000) return;
    lastOver = t;
    lastAt = Date.now();
    // (A card form's field: the menu offers Passwords' cards.)
    tell({ type: "focus", card: !!cardKind(t) });
  };
  document.addEventListener("focusin", focused, true);
  document.addEventListener("pointerover", focused, true);
  document.addEventListener("mousedown", (e) => e.button === 2 && focused(e), true);

  // A page with fields: its logins are fetched ahead, and a sign-in begun
  // on the page before goes on here.
  if (document.querySelector("input") || deep(document, "input").length) {
    tell({ type: "page" }).then((r) => {
      if (r && r.watch) {
        watch();
        if (passwordField() || otpField()) tell({ type: "step" });
      }
    });
  }

  // A login sent: the username and password of a form with a filled
  // password field, when it is submitted, a submit button is pressed or
  // Enter is pressed in it. Reported once per value.
  let reported = "";
  function report() {
    const pass = all("input[type=password]").find((p) => p.value && !isOtp(p));
    if (!pass) return;
    const user = userField(pass);
    // (A sign-in in steps keeps the account in a hidden field for
    // password managers.)
    const hidden = deep(pass.form || document, "input[type=hidden],input[hidden],input[aria-hidden=true]").find(
      (e) => e.value && /username|email|identifier|login|user/.test(says(e)),
    );
    const username = user?.value || hidden?.value || "";
    const key = username + "\n" + pass.value;
    if (key === reported) return;
    reported = key;
    tell({ type: "submitted", username, password: pass.value });
  }
  document.addEventListener("submit", () => report(), true);
  document.addEventListener("click", (e) => {
    const t = e.composedPath ? e.composedPath()[0] : e.target;
    const b = t.closest && t.closest("button,input[type=submit],input[type=image],[role=button]");
    if (b) report();
  }, true);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && e.target instanceof HTMLInputElement) report();
  }, true);
})();
