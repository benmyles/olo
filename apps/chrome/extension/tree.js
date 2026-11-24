// The page as a widget tree, for agents (Agent Screens' `tree`): what a
// person can see and use on it, named as assistive technology names it,
// with its place in the page's viewport (CSS pixels). Injected into every
// frame of a tab on demand (background.js); its value is the result.
// Each iframe is told a token of its own (by postMessage, kept per
// element), which its frame reports, so background.js can put the
// frame's nodes inside the iframe's. Passwords' values are never read.

(async () => {
  const MAX = 4000;
  const TEXT_MAX = 300;
  // A paragraph is read whole (an agent quotes from it): what a page
  // says isn't a control's name. Cut only where it is no paragraph.
  const READ_MAX = 20000;
  // How long a frame waits to be told which iframe it is.
  const TOLD_WAIT = 150;
  const nodes = [];
  let next = 1;

  // Which iframe this frame is (content.js listens too, so it's usually
  // known already).
  if (window !== window.top && !window.__oloFrame) {
    await new Promise((done) => {
      const told = (e) => {
        if (e.source === window.parent && typeof e.data?.__oloFrame === "string") {
          window.__oloFrame = e.data.__oloFrame;
          done();
        }
      };
      addEventListener("message", told);
      setTimeout(done, TOLD_WAIT);
    });
  }

  // The token of each iframe in this frame, the same each time; told to
  // its frame again each time (it may have navigated since).
  const frames = (window.__oloFrames ||= { next: 0, tokens: new WeakMap() });
  function frameToken(e) {
    let t = frames.tokens.get(e);
    if (!t) {
      t = `${Math.random().toString(36).slice(2)}.${++frames.next}`;
      frames.tokens.set(e, t);
    }
    try {
      e.contentWindow?.postMessage({ __oloFrame: t }, "*");
    } catch {}
    return t;
  }

  // An iframe's content box (its frame's viewport), in this frame's.
  function inside(e, r) {
    const st = getComputedStyle(e);
    const px = (p) => parseFloat(st[p]) || 0;
    const left = px("borderLeftWidth") + px("paddingLeft");
    const top = px("borderTopWidth") + px("paddingTop");
    return {
      x: r.left + left,
      y: r.top + top,
      width: Math.max(0, r.width - left - px("borderRightWidth") - px("paddingRight")),
      height: Math.max(0, r.height - top - px("borderBottomWidth") - px("paddingBottom")),
    };
  }

  const INPUT_ROLES = {
    checkbox: "CheckBox", radio: "RadioButton", range: "Slider", button: "Button", submit: "Button",
    reset: "Button", image: "Button", file: "Button", color: "ColorWell", search: "SearchInput",
  };
  const ARIA_ROLES = {
    button: "Button", link: "Link", checkbox: "CheckBox", switch: "Switch", radio: "RadioButton",
    textbox: "TextInput", searchbox: "SearchInput", combobox: "ComboBox", listbox: "ListBox",
    option: "ListBoxOption", menu: "Menu", menubar: "MenuBar", menuitem: "MenuItem",
    menuitemcheckbox: "MenuItemCheckBox", menuitemradio: "MenuItemRadio", tab: "Tab", tablist: "TabList",
    tabpanel: "TabPanel", slider: "Slider", spinbutton: "SpinButton", heading: "Heading", img: "Image",
    dialog: "Dialog", alertdialog: "AlertDialog", alert: "Alert", grid: "Grid", table: "Table",
    row: "Row", cell: "Cell", gridcell: "Cell", columnheader: "ColumnHeader", rowheader: "RowHeader",
    list: "List", listitem: "ListItem", tree: "Tree", treeitem: "TreeItem", navigation: "Navigation",
    main: "Main", banner: "Banner", search: "Search", form: "Form", progressbar: "ProgressIndicator",
    status: "Status",
  };
  // Containers kept for their structure (they hold what's inside).
  const LANDMARKS = new Set(["Dialog", "AlertDialog", "Navigation", "Main", "Banner", "Search", "Form", "Menu", "MenuBar", "TabList", "List", "ListBox", "Table", "Grid", "Tree", "Row"]);

  const clip = (s) => {
    s = (s || "").replace(/\s+/g, " ").trim();
    return s.length > TEXT_MAX ? s.slice(0, TEXT_MAX - 1) + "…" : s;
  };

  function roleOf(e) {
    const aria = (e.getAttribute("role") || "").split(/\s+/)[0];
    if (ARIA_ROLES[aria]) return ARIA_ROLES[aria];
    const tag = e.localName;
    switch (tag) {
      case "a": return e.hasAttribute("href") ? "Link" : null;
      case "button": case "summary": return "Button";
      case "input": {
        const t = (e.type || "text").toLowerCase();
        if (t === "hidden") return null;
        return INPUT_ROLES[t] || (t === "password" ? "PasswordInput" : "TextInput");
      }
      case "textarea": return "MultilineTextInput";
      case "select": return e.multiple ? "ListBox" : "ComboBox";
      case "option": return "ListBoxOption";
      case "img": return e.alt ? "Image" : null;
      case "h1": case "h2": case "h3": case "h4": case "h5": case "h6": return "Heading";
      case "dialog": return "Dialog";
      case "nav": return "Navigation";
      case "main": return "Main";
      case "form": return "Form";
      case "table": return "Table";
      case "tr": return "Row";
      case "td": return "Cell";
      case "th": return "ColumnHeader";
      case "ul": case "ol": return "List";
      case "li": return "ListItem";
      case "label": return "Label";
      case "progress": return "ProgressIndicator";
      case "video": case "audio": return "Video";
      case "iframe": case "frame": return "Iframe";
    }
    if (e.isContentEditable && (!e.parentElement || !e.parentElement.isContentEditable)) return "MultilineTextInput";
    return null;
  }

  function textOf(id) {
    const e = document.getElementById(id);
    return e ? e.innerText || e.textContent : "";
  }

  function nameOf(e, role) {
    const by = e.getAttribute("aria-labelledby");
    if (by) {
      const t = clip(by.split(/\s+/).map(textOf).join(" "));
      if (t) return t;
    }
    const aria = clip(e.getAttribute("aria-label"));
    if (aria) return aria;
    if (e.labels && e.labels.length) {
      const t = clip(Array.from(e.labels).map((l) => l.innerText).join(" "));
      if (t) return t;
    }
    if (e.localName === "img") return clip(e.alt);
    if (e.localName === "input" && ["button", "submit", "reset"].includes(e.type)) return clip(e.value || e.type);
    if (e.localName === "input" && e.type === "image") return clip(e.alt || e.title);
    if (/Input|ComboBox|ListBox|Slider|SpinButton/.test(role)) return clip(e.placeholder || e.title);
    if (LANDMARKS.has(role) && role !== "Row") return clip(e.title);
    if (role === "Iframe") return clip(e.title || e.name);
    return clip(e.innerText || e.textContent || e.title);
  }

  function valueOf(e, role) {
    if (role === "PasswordInput") return e.value ? "••••••••" : "";
    if (e.localName === "select") return clip(Array.from(e.selectedOptions).map((o) => o.text).join(", "));
    if (e.localName === "input" || e.localName === "textarea") {
      if (["checkbox", "radio", "button", "submit", "reset", "image", "file"].includes(e.type)) return "";
      return clip(e.value);
    }
    if (e.isContentEditable) return clip(e.innerText);
    const now = e.getAttribute("aria-valuetext") || e.getAttribute("aria-valuenow");
    return clip(now);
  }

  function clickable(e, role) {
    if (/Clickable|Button|Link|CheckBox|RadioButton|Switch|Tab|MenuItem|ListBoxOption|TreeItem|ComboBox|Input|Slider|SpinButton/.test(role)) return true;
    if (e.hasAttribute("onclick") || e.hasAttribute("tabindex") && e.tabIndex >= 0) return true;
    return getComputedStyle(e).cursor === "pointer";
  }

  // The layer of the page's own that `h` is in, if any: what is drawn
  // over the page as it scrolls (a sticky header, a fixed banner, a
  // dialog's backdrop).
  function layerOf(h) {
    for (let a = h; a && a !== document.documentElement; a = a.parentElement) {
      const p = getComputedStyle(a).position;
      if (p === "fixed" || p === "sticky") return a;
    }
    return null;
  }

  // What a click at the middle of `e` would reach instead, where a layer
  // of the page's own lies over it there (a page's sticky header over
  // what was scrolled under it: a click "pressed" a button it never
  // reached, an agent's report, 2026-11-24). Only such a layer, not
  // holding `e` itself: a control drawn over its own input (a check
  // box's box, a field's floating label) is that control to a person.
  function coveredBy(e, r) {
    const x = r.left + r.width / 2;
    const y = r.top + r.height / 2;
    if (x < 0 || y < 0 || x >= innerWidth || y >= innerHeight) return null;
    const root = e.getRootNode();
    const h = (root.elementFromPoint ? root : document).elementFromPoint(x, y);
    if (!h || h === e || e.contains(h) || h.contains(e)) return null;
    if (h.closest("label")?.control === e) return null;
    const layer = layerOf(h);
    if (!layer || layer.contains(e)) return null;
    const role = roleOf(layer);
    const name = role ? nameOf(layer, role) : "";
    const kind = role || (layer.localName === "header" || layer.localName === "footer" ? layer.localName : "layer");
    return `${getComputedStyle(layer).position} ${kind}${name ? ` "${clip(name)}"` : ""}`;
  }

  function shown(e, r) {
    if (r.width <= 0 && r.height <= 0) return false;
    const st = getComputedStyle(e);
    return st.visibility !== "hidden" && st.display !== "none" && e.getAttribute("aria-hidden") !== "true";
  }

  const inline = (e) => {
    const d = getComputedStyle(e).display;
    return d.startsWith("inline") || d === "contents";
  };

  // Text not inside anything named: what a person reads on the page.
  // Where an element has words of its own, they are read with the
  // words of what runs in the line with them (a link, bold, a span),
  // in their places: a paragraph is what the page says, whole. (Its own
  // text nodes alone left out every word that was a link: "Nvidia on
  // Monday announced Huang has been reinstated" came back as "on Monday
  // announced has been ,", and anything past 300 letters as "…": an
  // agent reading a news page through Rex, 2026-11-23.) A control in
  // the line that isn't a link (a button, a field) is its own node and
  // its words aren't the paragraph's; nor is what stands in a block of
  // its own inside it, which is read by itself.
  function lineText(e) {
    let t = "";
    for (const c of e.childNodes) {
      if (c.nodeType === Node.TEXT_NODE) t += c.textContent;
      else if (c.nodeType === Node.ELEMENT_NODE) {
        if (c.localName === "br") t += " ";
        else if (!shown(c, c.getBoundingClientRect()) && getComputedStyle(c).display !== "contents") continue;
        else if (!inline(c)) t += " ";
        else {
          const role = roleOf(c);
          if (!role || role === "Link" || role === "Label") t += lineText(c);
        }
      }
    }
    return t;
  }

  function directText(e) {
    let own = false;
    for (const c of e.childNodes) if (c.nodeType === Node.TEXT_NODE && c.textContent.trim()) own = true;
    if (!own) return "";
    const s = lineText(e).replace(/\s+/g, " ").trim();
    return s.length > READ_MAX ? s.slice(0, READ_MAX - 1) + "…" : s;
  }

  function push(e, r, role, name, parent, extra) {
    const id = next++;
    const n = { id, role, x: r.left, y: r.top, width: r.width, height: r.height, enabled: true };
    if (parent) n.parent = parent;
    if (name) n.name = name;
    Object.assign(n, extra);
    nodes.push(n);
    return id;
  }

  // Something without a role that takes clicks: script-made buttons.
  function generic(e) {
    if (e.hasAttribute("onclick") || (e.hasAttribute("tabindex") && e.tabIndex >= 0)) return true;
    const up = e.parentElement;
    return getComputedStyle(e).cursor === "pointer" && (!up || getComputedStyle(up).cursor !== "pointer");
  }

  // `covered`: its words are in the label of what it runs in the line
  // of already (they aren't said again; a link in it is still a node).
  function walk(e, parent, inNamed, covered) {
    if (nodes.length >= MAX) return;
    const r = e.getBoundingClientRect();
    let role = roleOf(e);
    if (!role && !inNamed && shown(e, r) && generic(e)) role = "Clickable";
    // display: contents and the like have no box but do have children.
    const visible = shown(e, r) || getComputedStyle(e).display === "contents";
    if (!visible) return;
    let mine = parent;
    let named = inNamed;
    // A control's label: its words are the control's name already.
    if (e.localName === "label" && e.control) named = true;
    if (role && role !== "Label" && shown(e, r)) {
      const extra = {};
      const disabled = e.disabled || e.getAttribute("aria-disabled") === "true";
      if (disabled) extra.enabled = false;
      if (clickable(e, role)) {
        extra.clickable = true;
        const over = coveredBy(e, r);
        if (over) extra.covered = over;
      }
      if (document.activeElement === e) extra.focused = true;
      if (e.localName === "input" && (e.type === "checkbox" || e.type === "radio")) extra.checked = e.checked;
      else if (e.hasAttribute("aria-checked")) extra.checked = e.getAttribute("aria-checked") === "true";
      if (e.hasAttribute("aria-selected")) extra.selected = e.getAttribute("aria-selected") === "true";
      else if (e.localName === "option") extra.selected = e.selected;
      // What a <select> offers: its menu is the browser's to draw, so its
      // options are no nodes of their own (agents choose by their words).
      if (e.localName === "select") extra.options = Array.from(e.options).slice(0, 60).map((o) => clip(o.text)).filter(Boolean);
      const value = valueOf(e, role);
      if (value) extra.value = value;
      if (role === "Iframe") {
        extra.frame = frameToken(e);
        extra.inner = inside(e, r);
      }
      const leaf = !LANDMARKS.has(role) && role !== "ListItem" && role !== "Cell" && role !== "ColumnHeader";
      mine = push(e, r, role, nameOf(e, role), parent, extra);
      // Inside a named control its words are its name already.
      if (leaf) named = true;
      // (And an item's or a cell's name is its words: what runs in
      // its line isn't said again under it; a link in it is a node.)
      else if (role === "ListItem" || role === "Cell" || role === "ColumnHeader") covered = true;
      if (role === "TextInput" || role === "PasswordInput" || role === "MultilineTextInput" || role === "ComboBox" || role === "Slider" || role === "Image" || role === "Iframe") return;
    } else if (!named && !covered && shown(e, r)) {
      const t = directText(e);
      if (t) {
        push(e, r, "Label", t, parent, {});
        covered = true;
      }
    }
    for (const c of e.children) walk(c, mine, named, covered && inline(c));
    if (e.shadowRoot) for (const c of e.shadowRoot.children) walk(c, mine, named, false);
  }

  if (document.body) walk(document.body, 0, false, false);
  return {
    title: document.title,
    url: location.href,
    width: innerWidth,
    height: innerHeight,
    nodes,
    truncated: nodes.length >= MAX,
    frame: window.__oloFrame || null,
  };
})();
