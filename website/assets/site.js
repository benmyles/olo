/* Olo Desktop's website: what the page does, all of it event-driven.
   Nothing runs while you read but the Deskbar's clock, once a minute
   (and not at all while the page is hidden). */
(function () {
  "use strict";
  var root = document.documentElement;
  root.classList.add("js");
  var $ = function (s, el) { return (el || document).querySelector(s); };
  var $$ = function (s, el) { return Array.prototype.slice.call((el || document).querySelectorAll(s)); };

  /* ---------- Light, dark, night ---------- */
  var system = matchMedia("(prefers-color-scheme: dark)");
  function stored() { try { return localStorage.getItem("olo-theme"); } catch (e) { return null; } }
  function showTheme(t) {
    root.dataset.theme = t;
    $$("[data-set-theme]").forEach(function (b) { b.setAttribute("aria-pressed", String(b.dataset.setTheme === t)); });
  }
  showTheme(root.dataset.theme || "light");
  $$("[data-set-theme]").forEach(function (b) {
    b.addEventListener("click", function () {
      var t = b.dataset.setTheme;
      try { localStorage.setItem("olo-theme", t); } catch (e) {}
      showTheme(t);
      if (root.dataset.view === "agent") rebuild();
    });
  });
  system.addEventListener("change", function () { if (!stored()) showTheme(system.matches ? "dark" : "light"); });

  /* ---------- Deskbar: its clock, by the minute ---------- */
  var date = $("#db-date"), time = $("#db-time"), tick = 0;
  // As Olo's Deskbar writes it: "Fri Nov 27".
  var dateFmt = { format: function (d) {
    var p = {};
    new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric" }).formatToParts(d).forEach(function (x) { p[x.type] = x.value; });
    return [p.weekday, p.month, p.day].join(" ");
  } };
  var timeFmt = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
  function clock() {
    var now = new Date();
    date.textContent = dateFmt.format(now);
    time.textContent = timeFmt.format(now);
    clearTimeout(tick);
    if (!document.hidden) tick = setTimeout(clock, 60000 - (now.getSeconds() * 1000 + now.getMilliseconds()) + 20);
  }
  clock();
  document.addEventListener("visibilitychange", function () { if (document.hidden) clearTimeout(tick); else clock(); });

  /* ---------- This page, measured as it loaded (the About window says it) ---------- */
  var seen = Object.create(null), bytes = 0, files = 0;
  function count(list) {
    list.forEach(function (e) {
      if (seen[e.name]) return;
      seen[e.name] = 1;
      var b = e.encodedBodySize || e.transferSize || 0;
      bytes += b; files += 1;
    });
    var kb = Math.max(1, Math.round(bytes / 1024));
    var w = kb >= 1024 ? (kb / 1024).toFixed(1) + " MB" : kb + " KB";
    $("#foot-weight").textContent = "This page, measured as it loaded in your browser: " + w + " in " + files + " files so far (pictures come as you reach them). Nothing runs while you read it but the Deskbar's clock, once a minute.";
  }
  if (window.performance && performance.getEntriesByType) {
    count(performance.getEntriesByType("navigation").concat(performance.getEntriesByType("resource")));
    if (window.PerformanceObserver) {
      try { new PerformanceObserver(function (l) { count(l.getEntries()); }).observe({ type: "resource", buffered: true }); } catch (e) {}
    }
  }

  /* ---------- Deskbar: open on the desktop, folded to its corner once a
     window is in front (Olo's ⌃⌘D), unless you open or fold it yourself ---------- */
  var bar = $("#deskbar"), fold = $(".db-fold"), wide = matchMedia("(min-width: 1240px)"), pastHero = false;
  function folded() { var f = bar.dataset.folded; return f === "true" || (f === "auto" && (!wide.matches || pastHero)); }
  function showFold() {
    var f = folded();
    bar.classList.toggle("is-folded", f);
    fold.setAttribute("aria-expanded", String(!f));
    fold.setAttribute("aria-label", f ? "Unfold Deskbar" : "Fold Deskbar");
    fold.title = fold.getAttribute("aria-label");
  }
  fold.addEventListener("click", function () { bar.dataset.folded = folded() ? "false" : "true"; showFold(); });
  wide.addEventListener("change", function () { bar.dataset.folded = "auto"; showFold(); });
  $$(".db-apps a, .db-mark").forEach(function (a) { a.addEventListener("click", function () { if (!wide.matches || pastHero) { bar.dataset.folded = "auto"; showFold(); } }); });
  document.addEventListener("keydown", function (e) { if (e.key === "Escape" && bar.dataset.folded === "false") { bar.dataset.folded = "auto"; showFold(); fold.focus(); } });
  showFold();

  /* ---------- The window in front: its tab yellow, its name bold in the Deskbar ---------- */
  var secs = $$(".sec"), links = $$(".db-apps a"), frontName = $("#db-front");
  function front(id) {
    var name = "Olo";
    secs.forEach(function (s) { s.classList.toggle("is-front", s.id === id); });
    links.forEach(function (a) {
      if (a.getAttribute("href") === "#" + id) { a.setAttribute("aria-current", "true"); name = a.textContent.trim(); }
      else a.removeAttribute("aria-current");
    });
    frontName.textContent = name === "Olo" ? "" : name;
  }
  if (window.IntersectionObserver) {
    // The window whose top part crosses the line 40% down the screen.
    // The hero is in that band too: there, no window is in front.
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) { if (e.isIntersecting) front(e.target.classList.contains("hero") ? secs[0].id : e.target.id); });
    }, { rootMargin: "-40% 0px -59% 0px" });
    secs.concat($(".hero")).forEach(function (s) { io.observe(s); });
    // Deskbar folds once the page has moved the desktop icons up to it
    // (the fold line, just above them, passes under its bottom edge).
    new IntersectionObserver(function (entries) {
      var e = entries[0];
      pastHero = !e.isIntersecting && e.boundingClientRect.top < 368;
      root.classList.toggle("past-hero", pastHero);
      showFold();
    }, { rootMargin: "-368px 0px 0px 0px" }).observe($("#fold-line"));
  }

  /* ---------- A menu bar in every window, its menus working ---------- */
  var openMenu = null;
  function closeMenus() { if (openMenu) { openMenu.classList.remove("open"); $(".menu-title", openMenu).setAttribute("aria-expanded", "false"); $(".menu-list", openMenu).hidden = true; openMenu = null; } }
  function openIt(m) { closeMenus(); openMenu = m; m.classList.add("open"); $(".menu-title", m).setAttribute("aria-expanded", "true"); var l = $(".menu-list", m); l.hidden = false; refresh(l); }
  function el(tag, cls, text) { var e = document.createElement(tag); if (cls) e.className = cls; if (text) e.textContent = text; return e; }
  function item(list, label, act, extra) {
    var b = el("button", "", label); b.type = "button"; b.setAttribute("role", extra && extra.check ? "menuitemradio" : "menuitem");
    if (extra && extra.key) { var k = el("kbd", "", extra.key); b.appendChild(k); }
    b.addEventListener("click", function () { closeMenus(); act(b); });
    if (extra && extra.check) b.check = extra.check;
    if (extra && extra.enabled) b.enabled = extra.enabled;
    list.appendChild(b); return b;
  }
  function refresh(list) {
    $$("button", list).forEach(function (b) {
      if (b.check) b.setAttribute("aria-checked", String(!!b.check()));
      if (b.enabled) b.disabled = !b.enabled();
    });
  }
  function menu(bar, title, fill) {
    var m = el("div", "menu"), t = el("button", "menu-title", title), l = el("div", "menu-list");
    t.type = "button"; t.setAttribute("aria-haspopup", "menu"); t.setAttribute("aria-expanded", "false");
    l.setAttribute("role", "menu"); l.hidden = true;
    t.addEventListener("click", function (e) { e.stopPropagation(); if (openMenu === m) closeMenus(); else openIt(m); });
    t.addEventListener("pointerenter", function () { if (openMenu && openMenu !== m) openIt(m); });
    fill(l); m.appendChild(t); m.appendChild(l); bar.appendChild(m);
  }
  var order = secs;
  function go(sec, step) { var i = order.indexOf(sec) + step; if (order[i]) order[i].scrollIntoView(); }
  function copyLink(sec, b) {
    var url = location.origin + location.pathname + "#" + sec.id;
    if (navigator.clipboard) navigator.clipboard.writeText(url).then(function () {}, function () {});
  }
  order.forEach(function (sec) {
    var w = $(".win", sec), tab = $(".win-tab", w), name = tab.textContent.trim();
    var bar = el("div", "menubar"); bar.setAttribute("role", "menubar"); bar.setAttribute("aria-label", name);
    menu(bar, name, function (l) {
      item(l, "Copy Link", function (b) { copyLink(sec, b); });
      item(l, "Agent View", function () { agent(true); });
    });
    if (sec.dataset.viewmenu === "xray") menu(bar, "View", function (l) {
      [["What You See", "person"], ["What It Reads", "agent"]].forEach(function (v) {
        item(l, v[0], function () { $('[data-xray="' + v[1] + '"]').click(); }, { check: function () { return (xray.dataset.mode || "person") === v[1]; } });
      });
    });
    if (sec.dataset.viewmenu === "looks") menu(bar, "View", function (l) {
      $$('input[name="look"]', sec).forEach(function (r) {
        item(l, $("b", r.parentNode).textContent, function () { r.checked = true; r.dispatchEvent(new Event("change", { bubbles: true })); }, { check: function () { return r.checked; } });
      });
    });
    menu(bar, "Go", function (l) {
      item(l, "Previous Window", function () { go(sec, -1); }, { enabled: function () { return order.indexOf(sec) > 0; } });
      item(l, "Next Window", function () { go(sec, 1); }, { enabled: function () { return order.indexOf(sec) < order.length - 1; } });
      l.appendChild(el("hr"));
      item(l, "Top of the Desktop", function () { scrollTo(0, 0); });
    });
    w.insertBefore(bar, $(".win-body", w));
  });
  (function () {
    var w = $(".av-win"); if (!w) return;
    var bar = el("div", "menubar"); bar.setAttribute("role", "menubar"); bar.setAttribute("aria-label", "Terminal");
    menu(bar, "Terminal", function (l) { item(l, "Back to the Page", function () { agent(false); }); });
    w.insertBefore(bar, $(".win-body", w));
  })();
  document.addEventListener("click", function (e) { if (openMenu && !openMenu.contains(e.target)) closeMenus(); });
  document.addEventListener("keydown", function (e) { if (e.key === "Escape" && openMenu) { var t = $(".menu-title", openMenu); closeMenus(); t.focus(); } });

  /* ---------- The plate's notes: pointed at, the legend's line lights;
     chosen, its words open in a balloon where it points, and the page stays put ---------- */
  var platePic = $(".plate-pic"), balloon = null, openPin = null;
  if (platePic) {
    balloon = el("div", "balloon"); balloon.setAttribute("role", "note"); balloon.id = "plate-balloon"; balloon.hidden = true;
    platePic.appendChild(balloon);
  }
  function closeBalloon(refocus) {
    if (!openPin) return;
    var pin = openPin; openPin = null;
    balloon.hidden = true;
    pin.setAttribute("aria-expanded", "false");
    pin.lit(false);
    if (refocus) pin.focus();
  }
  $$(".note").forEach(function (pin) {
    var note = $(pin.getAttribute("href"));
    pin.lit = function (on) { var keep = on || openPin === pin; pin.classList.toggle("lit", keep); note.classList.toggle("lit", keep); };
    [pin, note].forEach(function (el) {
      el.addEventListener("pointerenter", function () { pin.lit(true); });
      el.addEventListener("pointerleave", function () { pin.lit(false); });
    });
    pin.addEventListener("focus", function () { pin.lit(true); });
    pin.addEventListener("blur", function () { pin.lit(false); });
    pin.setAttribute("aria-controls", "plate-balloon");
    pin.setAttribute("aria-expanded", "false");
    pin.addEventListener("click", function (e) {
      e.preventDefault();
      if (openPin === pin) { closeBalloon(false); return; }
      closeBalloon(false);
      // Where its line ends, the thing it points at: in the plate's percent.
      var x = parseFloat(pin.style.getPropertyValue("--x")), y = parseFloat(pin.style.getPropertyValue("--y"));
      balloon.innerHTML = note.innerHTML;
      balloon.style.setProperty("--bx", x + "%");
      balloon.style.setProperty("--by", y + "%");
      balloon.classList.toggle("fx", x > 55);
      balloon.classList.toggle("fy", y > 55);
      balloon.hidden = false;
      openPin = pin;
      pin.setAttribute("aria-expanded", "true");
      pin.lit(true);
    });
  });
  document.addEventListener("click", function (e) { if (openPin && !e.target.closest(".note, .balloon")) closeBalloon(false); });
  document.addEventListener("keydown", function (e) { if (e.key === "Escape" && openPin) closeBalloon(true); });

  /* ---------- Two readers: DeskCalc and its tree ---------- */
  var xray = $("#xray");
  if (xray) {
    var NS = "http://www.w3.org/2000/svg", svg = $("#xray-boxes"), ox = 48, oy = 40; // the window's frame on its screen
    var re = /^\s*(\w+) "([^"]*)"(?: value="[^"]*")? at (\d+),(\d+) (\d+)x(\d+)/;
    var boxes = {}, lines = {};
    function box(x, y, w, h, name, cls) {
      var g = document.createElementNS(NS, "g"), r = document.createElementNS(NS, "rect"), t = document.createElementNS(NS, "text");
      r.setAttribute("x", x - ox + .5); r.setAttribute("y", y - oy + .5); r.setAttribute("width", w - 1); r.setAttribute("height", h - 1); r.setAttribute("rx", 2);
      t.setAttribute("x", x - ox + w / 2); t.setAttribute("y", y - oy + h / 2); t.textContent = name;
      if (cls) g.setAttribute("class", cls);
      g.appendChild(r); g.appendChild(t); svg.appendChild(g);
      return g;
    }
    box(53, 66, 266, 238, "", "win-box");
    $$("#xray-tree [data-i]").forEach(function (span) {
      var i = span.dataset.i, m = re.exec(span.textContent);
      (lines[i] = lines[i] || []).push(span);
      if (!m || boxes[i]) return;
      var g = boxes[i] = box(+m[3], +m[4], +m[5], +m[6], m[1] === "TextInput" ? "Expression" : m[2]);
      g.dataset.i = i; g.dataset.role = m[1]; g.dataset.name = m[2];
    });
    function lit(i, on) {
      if (boxes[i]) boxes[i].classList.toggle("lit", on);
      (lines[i] || []).forEach(function (s) { s.classList.toggle("lit", on); });
    }
    svg.addEventListener("pointerover", function (e) { var g = e.target.closest("g[data-i]"); if (g) lit(g.dataset.i, true); });
    svg.addEventListener("pointerout", function (e) { var g = e.target.closest("g[data-i]"); if (g) lit(g.dataset.i, false); });
    $("#xray-tree").addEventListener("pointerover", function (e) { var s = e.target.closest("[data-i]"); if (s) lit(s.dataset.i, true); });
    $("#xray-tree").addEventListener("pointerout", function (e) { var s = e.target.closest("[data-i]"); if (s) lit(s.dataset.i, false); });
    $$("[data-xray]").forEach(function (b) {
      b.addEventListener("click", function () {
        xray.dataset.mode = b.dataset.xray;
        $$("[data-xray]").forEach(function (o) { o.setAttribute("aria-pressed", String(o === b)); });
      });
    });
  }

  /* ---------- Apps: Tracker's status line says what the one you point at does ---------- */
  var about = $("#ts-about"), plain = about ? about.innerHTML : "", chosen = null;
  function say(btn) {
    if (chosen) chosen.classList.remove("chosen");
    chosen = btn;
    if (!btn) { about.innerHTML = plain; return; }
    btn.classList.add("chosen");
    about.innerHTML = "";
    var b = document.createElement("b"); b.textContent = btn.textContent.trim();
    about.appendChild(b); about.appendChild(document.createTextNode(": " + btn.dataset.about));
  }
  $$("#app-icons button").forEach(function (btn) {
    btn.addEventListener("pointerenter", function () { say(btn); });
    btn.addEventListener("focus", function () { say(btn); });
    btn.addEventListener("click", function () { say(btn); });
  });

  /* ---------- Apps: an app chosen opens its Get Info window, in front ---------- */
  var dlg = $("#app-info");
  if (dlg && dlg.showModal) {
    var appBtns = $$("#app-icons button[data-app]"), cur = 0;
    var showApp = function (i) {
      cur = (i + appBtns.length) % appBtns.length;
      var b = appBtns[cur], slug = b.dataset.app, name = $("span", b).textContent, w = +b.dataset.w, h = +b.dataset.h;
      $("#ai-tab").textContent = name;
      $("#ai-name").textContent = name;
      $("#ai-count").textContent = "Application, " + (cur + 1) + " of " + appBtns.length;
      $("#ai-icon").src = $("img", b).getAttribute("src");
      $("#ai-about").textContent = b.dataset.about;
      $("#ai-more").innerHTML = b.nextElementSibling.innerHTML;
      var shot = $("#ai-shot"), img = $("#ai-shot-img");
      shot.hidden = !w;
      if (w) {
        // In the page's own mode (Deskbar's switches are behind this window while it's open).
        var mode = root.dataset.theme === "light" ? "light" : "dark", base = "assets/img/app-" + slug + "-" + mode;
        shot.style.setProperty("--w", w + "px");
        shot.style.setProperty("--h", h + "px");
        img.removeAttribute("src");
        img.width = w; img.height = h;
        img.srcset = base + "-1x.webp 1x, " + base + "-2x.webp 2x";
        img.src = base + "-1x.webp";
        img.alt = name + (mode === "dark" ? ", in dark mode," : "") + " on made-up data: " + b.dataset.shot;
      }
      if (dlg.open) frontName.textContent = name;
      say(b);
    };
    var closeApp = function () { dlg.close(); };
    appBtns.forEach(function (b, i) {
      b.addEventListener("click", function () { showApp(i); root.classList.add("has-dialog"); dlg.showModal(); frontName.textContent = $("span", b).textContent; });
    });
    $("#ai-prev").addEventListener("click", function () { showApp(cur - 1); });
    $("#ai-next").addEventListener("click", function () { showApp(cur + 1); });
    $("#ai-close").addEventListener("click", closeApp);
    $(".ai-close-box").addEventListener("click", closeApp);
    // A click on the dimmed desktop round it closes it, as Escape does.
    dlg.addEventListener("click", function (e) { if (e.target === dlg) closeApp(); });
    dlg.addEventListener("keydown", function (e) {
      if (e.key === "ArrowLeft") { e.preventDefault(); showApp(cur - 1); }
      if (e.key === "ArrowRight") { e.preventDefault(); showApp(cur + 1); }
    });
    dlg.addEventListener("close", function () {
      root.classList.remove("has-dialog");
      var f = $(".sec.is-front");
      frontName.textContent = f ? ($('.db-apps a[href="#' + f.id + '"]') || { textContent: "" }).textContent.trim() : "";
    });
  }

  /* ---------- Looks: a look chosen brings its own icon set; any set can be chosen ---------- */
  var iconset = $("#iconset");
  if (iconset) {
    var setFor = { classic: "classic", platinum: "platinum", beos: "beos", amigaos: "amiga", aqua: "aqua", "tokyo-night": "classic" };
    var aboutSet = function (look) {
      var r = $('input[name="iconset"]:checked'), name = $("b", r.parentNode).textContent, about = $("#iconset-about");
      $("#iconset-icons").setAttribute("aria-label", "Sixteen of Olo's icons, in the " + name + " set");
      about.innerHTML = "";
      about.appendChild(el("b", "", name + "."));
      about.appendChild(document.createTextNode(" " + (look === "tokyo-night" ? "Tokyo Night wears Classic's icons, as every colour scheme does. " : "") + r.dataset.about));
    };
    $$('input[name="iconset"]').forEach(function (r) { r.addEventListener("change", function () { aboutSet(); }); });
    $$('input[name="look"]').forEach(function (r) {
      r.addEventListener("change", function () {
        if (!r.checked) return;
        var set = $('input[name="iconset"][value="' + (setFor[r.value] || "classic") + '"]');
        set.checked = true;
        aboutSet(r.value);
      });
    });
  }

  /* ---------- Copy ---------- */
  $$("[data-copy]").forEach(function (b) {
    b.addEventListener("click", function () {
      var text = $("#" + b.dataset.copy).textContent.replace(/^\$ /gm, "");
      var done = function () { b.textContent = "Copied"; setTimeout(function () { b.textContent = "Copy"; }, 1600); };
      if (navigator.clipboard) navigator.clipboard.writeText(text).then(done, function () {});
    });
  });

  /* ---------- Agent view: this page as olo-screen tree gives a web page ---------- */
  var view = $("#agent-view"), out = $("#av-tree"), toggle = $("#agent-toggle");
  var MAX = 84;
  function cut(s) { s = s.replace(/\s+/g, " ").trim(); return s.length > MAX ? s.slice(0, MAX - 1) + "…" : s; }
  function esc(s) { return s.replace(/[&<>"]/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]; }); }
  function shown(el) { return el.getClientRects().length > 0 && getComputedStyle(el).visibility !== "hidden"; }
  function where(el) {
    var r = el.getBoundingClientRect();
    return " at " + Math.round(r.left + scrollX) + "," + Math.round(r.top + scrollY) + " " + Math.round(r.width) + "x" + Math.round(r.height);
  }
  function nameOf(el) {
    var l = el.getAttribute("aria-label");
    if (l) return l;
    var by = el.getAttribute("aria-labelledby");
    if (by && document.getElementById(by)) return document.getElementById(by).textContent;
    return el.textContent;
  }
  var rows = [];
  function line(depth, role, name, el, extra, href) {
    var pad = new Array(depth + 1).join("  ");
    var n = name == null ? "" : ' <span class="n">"' + esc(cut(name)) + '"</span>';
    var body = '<span class="r">' + role + "</span>" + n + (extra ? " " + esc(extra) : "") + '<span class="g">' + where(el) + "</span>";
    if (href) body = '<a href="' + esc(href) + '">' + body + "</a>";
    rows.push(pad + body);
  }
  var LEAF_BLOCKS = "h1,h2,h3,h4,h5,h6,p,pre,figcaption,ol,ul,dl,figure,nav,section,header,div,details,label,img,button,a,input,summary,svg";
  function walk(el, depth) {
    Array.prototype.forEach.call(el.children, function (c) {
      if (c.hidden || c.getAttribute("aria-hidden") === "true" || c.matches(".agent-view, .skip, .night-light, script, style")) return;
      if (!shown(c) && !c.matches("img")) return;
      var tag = c.tagName.toLowerCase(), d = depth + 1;
      if (/^h[1-6]$/.test(tag)) { line(depth, "Heading", c.textContent, c, "level=" + tag[1]); return; }
      if (tag === "a") { line(depth, "Link", nameOf(c), c, "clickable", c.getAttribute("href")); return; }
      if (tag === "button" || tag === "summary") {
        var p = c.getAttribute("aria-pressed");
        line(depth, "Button", nameOf(c), c, (p === "true" ? "pressed " : "") + "clickable"); return;
      }
      if (tag === "img") { if (c.alt && shown(c)) line(depth, "Image", c.alt, c); return; }
      if (tag === "input" && c.type === "radio") { line(depth, "RadioButton", (c.parentNode.innerText || c.parentNode.textContent), c.parentNode, (c.checked ? "checked " : "") + "clickable"); return; }
      if (tag === "input" && c.type === "checkbox") { line(depth, "CheckBox", (c.parentNode.innerText || c.parentNode.textContent), c.parentNode, (c.checked ? "checked " : "") + "clickable"); return; }
      if (tag === "label" && c.querySelector("input")) { walk(c, depth); return; }
      if (tag === "pre") { line(depth, "Code", c.textContent.split("\n")[0], c); return; }
      if (tag === "p" || tag === "dd" || tag === "dt") {
        line(depth, "Paragraph", c.textContent, c);
        $$("a", c).forEach(function (a) { line(d, "Link", nameOf(a), a, "clickable", a.getAttribute("href")); });
        $$("button", c).forEach(function (b) { line(d, "Button", nameOf(b), b, "clickable"); });
        return;
      }
      if (tag === "li") {
        if (c.querySelector(LEAF_BLOCKS.replace(",a,", ",").replace(",button,", ","))) { line(depth, "ListItem", null, c); walk(c, d); }
        else if (c.querySelector("button, a") && !c.querySelector("p")) { walk(c, depth); }
        else { line(depth, "ListItem", c.textContent, c); $$("a", c).forEach(function (a) { line(d, "Link", nameOf(a), a, "clickable", a.getAttribute("href")); }); }
        return;
      }
      var role = { header: "Banner", main: "Main", nav: "Navigation", aside: "Complementary", footer: "ContentInfo", section: "Region", figure: "Figure", ol: "List", ul: "List", dl: "List", details: "Group" }[tag];
      if (role) { line(depth, role, (tag === "section" || tag === "nav" || tag === "aside") ? nameOf(c) : null, c); walk(c, d); return; }
      walk(c, depth);
    });
  }
  function build() {
    rows = [];
    var title = document.title;
    rows.push('<span class="c"># The page you were reading, as olo-screen tree gives a web page to an agent:</span>');
    rows.push('<span class="c"># each thing\'s role, its name, and where it is on the page, in pixels.</span>');
    rows.push('<span class="c"># The underlined names are links, and work here too.</span>');
    rows.push('<span class="g">$</span> <span class="cmd">olo-screen tree visitor</span>');
    rows.push('window 1 <span class="n">"' + esc(title) + '"</span> (browser) focused');
    rows.push('  <span class="r">Document</span> <span class="n">"' + esc(cut(title)) + '"</span> url="' + esc(location.origin + location.pathname) + '"');
    walk(document.body, 2);
    out.innerHTML = rows.join("\n");
  }
  var keptY = 0;
  // Read the page where it is (shown for a moment), as the tree says where things are.
  function rebuild() {
    delete root.dataset.view; view.hidden = true;
    scrollTo(0, keptY);
    build();
    root.dataset.view = "agent"; view.hidden = false;
    scrollTo(0, 0);
  }
  function agent(on) {
    toggle.checked = on;
    if (on) {
      keptY = scrollY;
      build();
      root.dataset.view = "agent";
      view.hidden = false;
      frontName.textContent = "Terminal";
      scrollTo(0, 0);
      $("#agent-back").focus({ preventScroll: true });
    } else {
      delete root.dataset.view;
      view.hidden = true;
      scrollTo(0, keptY);
    }
  }
  toggle.addEventListener("change", function () { agent(toggle.checked); });
  $("#agent-back").addEventListener("click", function () { agent(false); });
  out.addEventListener("click", function (e) {
    var a = e.target.closest("a");
    if (!a) return;
    var href = a.getAttribute("href");
    if (href && href.charAt(0) === "#") { e.preventDefault(); agent(false); var t = $(href); if (t) t.scrollIntoView(); history.replaceState(null, "", href); }
  });
})();
