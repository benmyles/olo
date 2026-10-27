// The page as a widget tree, for agents (Agent Screens' `tree`): what a
// person can see and use on it, named as assistive technology names it,
// with its place in the page's viewport (CSS pixels). Injected into a
// tab's top frame on demand (background.js); its value is the result.
// Passwords' values are never read.

(() => {
  const MAX = 4000;
  const TEXT_MAX = 300;
  const nodes = [];
  let next = 1;

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
      case "iframe": return "Iframe";
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

  function shown(e, r) {
    if (r.width <= 0 && r.height <= 0) return false;
    const st = getComputedStyle(e);
    return st.visibility !== "hidden" && st.display !== "none" && e.getAttribute("aria-hidden") !== "true";
  }

  // Text not inside anything named: what a person reads on the page.
  function directText(e) {
    let t = "";
    for (const c of e.childNodes) if (c.nodeType === Node.TEXT_NODE) t += c.textContent;
    return clip(t);
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

  function walk(e, parent, inNamed) {
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
      if (clickable(e, role)) extra.clickable = true;
      if (document.activeElement === e) extra.focused = true;
      if (e.localName === "input" && (e.type === "checkbox" || e.type === "radio")) extra.checked = e.checked;
      else if (e.hasAttribute("aria-checked")) extra.checked = e.getAttribute("aria-checked") === "true";
      if (e.hasAttribute("aria-selected")) extra.selected = e.getAttribute("aria-selected") === "true";
      else if (e.localName === "option") extra.selected = e.selected;
      const value = valueOf(e, role);
      if (value) extra.value = value;
      const leaf = !LANDMARKS.has(role) && role !== "ListItem" && role !== "Cell" && role !== "ColumnHeader";
      mine = push(e, r, role, nameOf(e, role), parent, extra);
      // Inside a named control its words are its name already.
      if (leaf) named = true;
      if (role === "TextInput" || role === "PasswordInput" || role === "MultilineTextInput" || role === "ComboBox" || role === "Slider" || role === "Image") return;
    } else if (!named && shown(e, r)) {
      const t = directText(e);
      if (t) push(e, r, "Label", t, parent, {});
    }
    for (const c of e.children) walk(c, mine, named);
    if (e.shadowRoot) for (const c of e.shadowRoot.children) walk(c, mine, named);
  }

  if (document.body) walk(document.body, 0, false);
  return {
    title: document.title,
    url: location.href,
    width: innerWidth,
    height: innerHeight,
    nodes,
    truncated: nodes.length >= MAX,
  };
})();
