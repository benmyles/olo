/* The User's Guide to Olo, opened over the desktop as a book (index.html's
   <dialog id="manual">). Its pages are assets/manual.html's, fetched with
   their stylesheet the first time the guide is opened, or an entry to it
   pointed at. Everything here answers an event: while the book lies open
   nothing runs; a page turning is the one thing that moves, and only while
   it turns. */
(function () {
  "use strict";
  var root = document.documentElement;
  var dlg = document.getElementById("manual");
  // Without a <dialog>, the entries are links to the guide's own page.
  if (!dlg || typeof dlg.showModal !== "function") return;
  var $ = function (s, el) { return (el || document).querySelector(s); };
  var $$ = function (s, el) { return Array.prototype.slice.call((el || document).querySelectorAll(s)); };
  var noop = function () {};

  var SRC = "assets/manual.html", CSS = "assets/manual.css";
  var ASPECT = 0.74;                        // a page's width to its height (manual.css --aspect)
  var TURN_MS = 680, WAIT_MS = 20000;
  var MAX_BYTES = 1 << 20;                  // the edition is about 110 KB: an answer past a megabyte isn't it
  var reduced = matchMedia("(prefers-reduced-motion: reduce)");
  var frame = $(".mn-frame", dlg), stage = $(".mn-stage", dlg), live = $(".mn-live", dlg);
  var progTop = $(".mn-top .mn-progress", dlg), progBot = $(".mn-bot .mn-progress", dlg);
  var plate = $(".mn-plate", dlg), plateBox = $(".mn-plate-view", dlg), plateImg = $(".mn-plate-view img", dlg), plateCap = $(".mn-plate figcaption", dlg);
  var plateBody = $(".mn-plate-body", dlg), plateName = $(".mn-plate-bar .mn-name span", dlg);
  var plateClose = $("[data-plate-close]", dlg), fsBtns = $$("[data-fullscreen]", dlg);
  var prevBtns = $$('[data-go="prev"]', dlg), nextBtns = $$('[data-go="next"]', dlg);

  var book = null, pages = [], N = 0, tabs = [], titles = {}, corners = {};
  var mode = "spread", fit = "fixed", cur = 1, want = 1, contents = 3, lastW = 0, lastH = 0;
  // `asked`: where the reader asked to open (an address, or a page), kept
  // until the pages are here. `req`: the open in hand; a newer one, or a
  // close, makes an older one void.
  var loading = null, attempt = null, asked = null, req = 0, slowT = 0, tries = 0, givingBack = false;
  var turning = null, opener = null, agentWas = false, agentRow = null, closing = false, navClosing = false, repush = false;
  // Open, and not yet tidied after closing. (The dialog's own close event
  // comes a task later: a close of ours is tidied at once, and the event,
  // finding it done or the guide open again, does nothing.)
  var wasOpen = false;

  /* ---------- Fetching the pages, the first time ---------- */
  // One attempt at the pages and their stylesheet, with one deadline for
  // both: at the deadline, at Close, or at any failure it is dropped whole
  // (the fetch aborted, a stylesheet still coming taken out so the next
  // attempt asks again, its listeners gone), and nothing is built after.
  function loadCss(a) {
    return new Promise(function (ok, no) {
      var l = $("link[data-manual-css]");
      if (l && l.dataset.loaded) return ok();
      if (l) l.remove();
      l = a.link = document.createElement("link");
      // (After a failure, under an address of its own: the browser would
      // join a new link to a request for the same address still pending.)
      l.rel = "stylesheet"; l.href = CSS + (tries ? "?try=" + tries : ""); l.setAttribute("data-manual-css", "");
      var done = a.unlisten = function () { l.removeEventListener("load", onLoad); l.removeEventListener("error", onError); };
      function onLoad() { done(); l.dataset.loaded = "1"; ok(); }
      function onError() { done(); l.remove(); no(new Error(CSS)); }
      l.addEventListener("load", onLoad);
      l.addEventListener("error", onError);
      document.head.appendChild(l);
    });
  }
  function fetchPages(a) {
    var how = {};
    if (a.ac) how.signal = a.ac.signal;
    // (Asked again after a failure: from the server, not a cache that may hold the failure.)
    if (tries) how.cache = "reload";
    return fetch(SRC, how).then(function (r) {
      if (!r.ok) throw new Error(SRC + ": " + r.status);
      if (+r.headers.get("content-length") > MAX_BYTES) throw new Error(SRC + ": too long");
      // Read in pieces, so that an answer past the cap stops there; a
      // browser that can't is offered the guide's own page instead.
      if (!r.body || !r.body.getReader || typeof TextDecoder !== "function") throw new Error(SRC + ": can't be read in pieces here");
      var rd = r.body.getReader(), dec = new TextDecoder(), parts = [], got = 0;
      return (function more() {
        return rd.read().then(function (x) {
          if (x.done) { parts.push(dec.decode()); return parts.join(""); }
          got += x.value.byteLength;
          if (got > MAX_BYTES) { rd.cancel().catch(noop); throw new Error(SRC + ": too long"); }
          parts.push(dec.decode(x.value, { stream: true }));
          return more();
        });
      })();
    });
  }
  function drop(a) {
    clearTimeout(a.timer);
    if (a.ac) a.ac.abort();
    if (a.unlisten) a.unlisten();
    if (a.link && !a.link.dataset.loaded && a.link.parentNode) a.link.remove();
    if (attempt === a) attempt = null;
  }
  function load() {
    if (loading) return loading;
    var a = attempt = { ac: typeof AbortController === "function" ? new AbortController() : null, over: false }, p;
    p = new Promise(function (ok, no) {
      // (Failed, it's no longer the one in hand, at once: an open in this
      // same moment starts afresh rather than on it.)
      a.fail = function (e) { if (a.over) return; a.over = true; tries++; drop(a); if (loading === p) loading = null; no(e); };
      a.timer = setTimeout(function () { a.fail(new Error(SRC + ": no answer in " + WAIT_MS / 1000 + " s")); }, WAIT_MS);
      Promise.all([loadCss(a), fetchPages(a)]).then(function (got) {
        if (a.over) return;
        build(got[1]);
        a.over = true;
        clearTimeout(a.timer);
        if (attempt === a) attempt = null;
        ok();
      }).catch(function (e) { a.fail(e); });
    });
    loading = p;
    // (Any failure, the stylesheet's, the pages', their building or the
    // deadline, is tried again next time: see a.fail.)
    p.catch(noop);
    return p;
  }
  // Closed while opening: that attempt is dropped, and builds nothing.
  function cancelLoad() { if (attempt && !book) attempt.fail(new Error("closed")); }
  // The guide's addresses are relative to its own page (assets/): made whole.
  function whole(u, base) { try { return new URL(u, base).href; } catch (e) { return u; } }
  function build(html) {
    var doc = new DOMParser().parseFromString(html, "text/html");
    var src = doc.getElementById("book");
    if (!src || !src.classList.contains("book")) throw new Error(SRC + ": no book in it");
    // Every page, in order, as many as the edition says, ending with the
    // back cover and its last word: the mark at its end, whole.
    var all = $$(".page", src), said = src.getAttribute("data-pages") || "", last = all[all.length - 1];
    var end = last && $("[data-end]", last), word = end ? end.textContent.trim() : "";
    var complete = /^[1-9][0-9]*$/.test(said) && +said === all.length && all.length >= 2
      && all.every(function (q, i) { return +q.getAttribute("data-page") === i + 1; })
      && last.classList.contains("cover") && word !== "" && word === end.getAttribute("data-end") && $$("*", last).pop() === end;
    if (!complete) throw new Error(SRC + ": not the whole guide");
    var base = new URL(SRC, location.href).href;
    $$("img", src).forEach(function (img) {
      if (img.hasAttribute("src")) img.setAttribute("src", whole(img.getAttribute("src"), base));
      if (img.hasAttribute("srcset")) img.setAttribute("srcset", img.getAttribute("srcset").split(",").map(function (c) {
        var p = c.trim().split(/\s+/);
        p[0] = whole(p[0], base);
        return p.join(" ");
      }).join(", "));
    });
    $$("a[href]", src).forEach(function (a) {
      var h = a.getAttribute("href");
      if (h.charAt(0) !== "#") a.setAttribute("href", whole(h, base));
      // (A picture's whole: its address, and its 1x and 2x, made whole too.)
      if (a.dataset.src) a.dataset.src = whole(a.dataset.src, base);
      if (a.dataset.srcset) a.dataset.srcset = a.dataset.srcset.split(",").map(function (c) {
        var p = c.trim().split(/\s+/);
        p[0] = whole(p[0], base);
        return p.join(" ");
      }).join(", ");
    });
    // Its pages' ids are the guide's own page's (its links go to them); here
    // an address with one would have the browser scroll the book to it.
    $$('[id^="manual/"]', src).forEach(function (e) { e.removeAttribute("id"); });
    var b = document.importNode(src, true);
    b.removeAttribute("data-fit");
    b.tabIndex = -1;
    var ps = $$(".page", b), ts = [], tt = {}, cs = {};
    ps.forEach(function (p) { p.hidden = true; });
    $$(".opener", b).forEach(function (p) { tt[p.dataset.ch] = ($("h2", p) || p).textContent.trim(); });
    // The book's shadow on the desk, its corners, and its chapters' edge tabs.
    b.appendChild(make("div", "shadow"));
    ["prev", "next"].forEach(function (d) {
      var c = cs[d] = make("button", "mn-corner " + d);
      c.type = "button";
      c.setAttribute("aria-label", d === "next" ? "Next page" : "Previous page");
      c.tabIndex = -1;                     // the arrows have the keyboard; these are for the pointer
      b.appendChild(c);
    });
    var holder = make("div", "mn-tabs");
    $$(".page[data-tab]", b).forEach(function (p, i) {
      // A chapter's tab at its opener, its number over its word; the index's, its word.
      var ch = p.classList.contains("opener") ? p.dataset.ch : "";
      var t = make("button", "mn-etab" + (ch ? "" : " ix"));
      t.type = "button";
      t.style.setProperty("--i", i);
      if (ch) t.dataset.ch = ch;
      t.dataset.page = p.dataset.page;
      t.dataset.name = p.dataset.tab;
      t.setAttribute("aria-label", (ch ? "Chapter " + ch + ", " + tt[ch] : p.dataset.tab) + ", page " + p.dataset.page);
      t.title = t.getAttribute("aria-label");
      if (ch) { var n = document.createElement("b"); n.textContent = ch; t.appendChild(n); }
      var w = document.createElement("small");
      w.textContent = p.dataset.tab;
      t.appendChild(w);
      holder.appendChild(t);
      ts.push(t);
    });
    b.appendChild(holder);
    var toc = $(".toc", b);
    // Built whole: only now is it the book.
    book = b; pages = ps; N = ps.length; tabs = ts; titles = tt; corners = cs;
    contents = toc ? num(toc.closest(".page")) : 3;
    $$(".book", stage).forEach(function (o) { o.remove(); });
    stage.appendChild(book);
  }
  function make(tag, cls) { var e = document.createElement(tag); e.className = cls; return e; }
  function num(p) { return +p.dataset.page; }
  function page(n) { return n >= 1 && n <= N ? pages[n - 1] : null; }
  function clamp(n) { return Math.max(1, Math.min(N, Math.floor(+n) || 1)); }

  /* ---------- How big, and how many pages at once ---------- */
  // Two pages where both fit at a size to read (a page 500 points wide or
  // more: its type at 12 points or more), with the bar above, and either
  // side the page edges, the edge tabs (one pulled out), 24 points and the
  // arrow; else one page, whole where it fits so, else set again to read,
  // as long as it is, and scrolled.
  function layout() {
    // The window's own box, open: on a phone a page wider than the screen
    // can make innerWidth the page's width rather than the screen's.
    var W = dlg.clientWidth || window.innerWidth, H = dlg.clientHeight || window.innerHeight;
    lastW = W; lastH = H;
    // Beside the book, each side: 16 to the screen's edge, a 36-point
    // arrow, 24 clear, 6 for a tab pulled out, and the tab and page edges
    // (2.82 of the page's own unit, a fortieth of its width).
    // (Above it the bar, 48 points, and 24 above and below it, room for a leaf as it lifts.)
    var pw = Math.min((H - 48 - 48) * ASPECT, (W - 2 * (16 + 36 + 24 + 6)) / (2 + 2 * 2.82 / 40), 680);
    if (pw >= 500) { mode = "spread"; fit = "fixed"; }
    else {
      // One page between the bars, as tall as they are here (a phone's
      // are taller, round its notch and home bar).
      mode = dlg.dataset.mode = "single";
      var bars = $(".mn-top", dlg).offsetHeight + $(".mn-bot", dlg).offsetHeight;
      pw = Math.min((H - bars - 24) * ASPECT, W - 48, 680);
      if (pw >= 500) fit = "fixed";
      else { fit = "flow"; pw = Math.min(W - (W < 520 ? 16 : 48), 640); }
    }
    pw = Math.floor(pw);
    dlg.dataset.mode = mode;
    dlg.dataset.fit = fit;
    book.dataset.mode = mode;
    book.dataset.fit = fit;
    book.style.setProperty("--pw", pw + "px");
    book.style.setProperty("--ph", Math.floor(pw / ASPECT) + "px");
    book.style.fontSize = fit === "flow" ? "" : pw / 40 + "px";
  }
  // Again, when the window's size has changed (and only then).
  function relayout(force) {
    if (!dlg.open || !book || dlg.dataset.state) return;
    if (!force && dlg.clientWidth === lastW && dlg.clientHeight === lastH) return;
    finishTurn();
    layout();
    place(want, false);
  }

  /* ---------- Which pages show ---------- */
  // The pages open with page n: its spread (left even, right odd; the
  // covers alone), or n alone.
  function shown(n) {
    if (mode === "single") return [n];
    var l = n - (n % 2);
    return [l, l + 1].filter(function (x) { return x >= 1 && x <= N; });
  }
  function sameView(a, b) { return shown(a)[0] === shown(b)[0]; }
  // The book open at page n (the page asked for, kept in the address: in a
  // spread, the one of its two pages that was asked for). `write` false:
  // the address is being followed (Back, Forward), so it stays as it is.
  function place(n, write) {
    n = clamp(n);
    var on = shown(n), was = cur;
    pages.forEach(function (p, i) {
      var show = on.indexOf(i + 1) >= 0;
      p.hidden = !show;
      p.classList.toggle("is-shown", show);
      p.classList.remove("is-under", "is-turning");
    });
    want = n;
    cur = on[0];
    // A page set to scroll starts at its top.
    if (fit === "flow" && cur !== was) stage.scrollTop = 0;
    settle(write);
  }
  // The edge tab whose pages hold page n (a chapter's, from its opener to
  // the next tab; the index's, to the back cover).
  function tabFor(n) {
    var t = null;
    tabs.forEach(function (x) { if (+x.dataset.page <= n) t = x; });
    if (t && page(N).classList.contains("cover") && n === N) return null;
    return t;
  }
  // Everything that follows from where the book is open.
  function settle(write) {
    var on = shown(cur), first = on[0], last = on[on.length - 1];
    if (mode === "spread") {
      book.dataset.at = first === 1 ? "front" : (on.length === 1 && first === N) ? "back" : "";
      // As many pages' edges on each side as have been read and are left.
      var u = parseFloat(book.style.fontSize) || 14;
      book.style.setProperty("--stack-l", Math.round(u * 0.72 * Math.max(0, first - 1) / N) + "px");
      book.style.setProperty("--stack-r", Math.round(u * 0.72 * Math.max(0, N - last) / N) + "px");
      tabs.forEach(function (t) {
        var left = +t.dataset.page <= first;
        t.classList.toggle("at-l", left);
        t.classList.toggle("at-r", !left);
      });
    } else book.dataset.at = "";
    var tab = tabFor(last) || tabFor(first), tl = tabFor(first);
    tabs.forEach(function (t) { t.setAttribute("aria-current", t === tab ? "true" : "false"); });
    corners.prev.hidden = first <= 1;
    corners.next.hidden = last >= N;
    prevBtns.forEach(function (b) { b.disabled = first <= 1; });
    nextBtns.forEach(function (b) { b.disabled = last >= N; });
    // What the bars say (the top one in full, the phone's bottom one short), and the address.
    var where = first === 1 ? "Front Cover" : (on.length === 1 && first === N && page(N).classList.contains("cover")) ? "Back Cover"
      : on.length === 2 ? "Pages " + first + "–" + last + " of " + N : "Page " + first + " of " + N;
    // (Under it, the chapter: two where a spread ends one and begins the
    // next, as 28–29, Quick Reference and the Index; the book's own name
    // on the covers and the pages before chapter 1.)
    var ch = tab && tab.dataset.ch;
    var named = function (t) { return t.dataset.ch ? "Chapter " + t.dataset.ch + ": " + titles[t.dataset.ch] : "Index and Colophon"; };
    var long = !tab ? "User’s Guide to Olo" : tl && tl !== tab ? named(tl) + " · " + (ch ? "Chapter " + ch : tab.dataset.name) : named(tab);
    var short = !tab ? "" : ch ? "Chapter " + ch + " · " + tab.dataset.name : tab.dataset.name;
    say(progTop, where, long);
    say(progBot, where, short);
    if (live) live.textContent = where + (long ? ". " + long : "");
    if (dlg.open && write !== false) history.replaceState(history.state, "", "#manual/p-" + want);
    // The pictures of the pages either side, fetched before they're turned to.
    prime(shown(Math.max(1, first - 1)).concat(on, shown(Math.min(N, last + 1))));
    // The keyboard stays where it was, unless that page has gone or that
    // button can't be pressed now (at either end): then the book has it.
    var a = document.activeElement;
    if (dlg.open && (!a || a === document.body || a === dlg || a.disabled || !dlg.contains(a) || (a.closest(".page") && a.closest(".page").hidden))) book.focus({ preventScroll: true });
  }
  function say(el, a, b) {
    if (!el) return;
    el.textContent = a;
    if (b) { var s = document.createElement("small"); s.textContent = b; el.appendChild(s); }
  }
  function prime(ns) {
    var dark = root.dataset.theme !== "light";
    ns.forEach(function (n) {
      var p = page(n);
      if (!p) return;
      // (Of the looks, only the one chosen: the others come when chosen.)
      var look = $('.looks-m input:checked', p);
      $$('img[loading="lazy"]', p).forEach(function (img) {
        if (img.closest(dark ? ".only-light" : ".only-dark")) return;
        var a = img.closest("[data-look]");
        if (a && (!look || a.dataset.look !== look.value)) return;
        img.loading = "eager";
      });
    });
  }

  /* ---------- Turning ---------- */
  // (`quiet`: landing it without writing the address, which is being followed.)
  function finishTurn(quiet) { if (turning) turning.end(quiet); }
  // To page n; `write` false while the address is being followed.
  function go(n, write) {
    if (!book || dlg.dataset.state || n == null || isNaN(n)) return;
    n = clamp(n);
    closePlate(false);
    finishTurn(write === false);
    if (sameView(n, cur)) { want = n; settle(write); return; }
    if (reduced.matches || fit === "flow" || typeof book.animate !== "function") { crossFade(n, write); return; }
    if (mode === "spread") turnSpread(n, write); else turnPage(n, write);
  }
  // (A turn still going lands first, so a quick second press turns on from there.)
  function next() { finishTurn(); var on = shown(cur); go(on[on.length - 1] + 1); }
  function prev() { finishTurn(); go(shown(cur)[0] - 1); }
  // A shade over a page as it lifts, or as the leaf's shadow falls on it.
  // (`under`: the shadow the lifting leaf casts on the page under it.)
  function shade(p, frames, ms, under) {
    var cls = under ? ".turn-shade.under" : ".turn-shade:not(.under)";
    var s = $(cls, p);
    if (!s) { s = make("i", "turn-shade" + (under ? " under" : "")); s.setAttribute("aria-hidden", "true"); p.appendChild(s); }
    return s.animate(frames, { duration: ms || TURN_MS, easing: "linear", fill: "both" });
  }
  // The bars, the tabs and the arrows say where the book is once the leaf
  // lands (and the address, if what turned it writes it).
  function run(to, write, parts) {
    var anims = [];
    var self = turning = {
      to: to,
      end: function (quiet) {
        if (turning !== self) return;
        turning = null;
        anims.forEach(function (a) { a.cancel(); });
        place(self.to, quiet ? false : write);
      }
    };
    parts(anims);
    anims[0].onfinish = function () { self.end(); };
  }
  // Two pages open: the leaf on the side we're going turns over the spine,
  // its back the next spread's page on the other side.
  function turnSpread(to, write) {
    var a = shown(cur), b = shown(to), fwd = to > cur;
    var al = a[0] % 2 === 0 ? page(a[0]) : null, ar = page(a[0] % 2 === 0 ? a[0] + 1 : a[0]);
    var bl = b[0] % 2 === 0 ? page(b[0]) : null, br = page(b[0] % 2 === 0 ? b[0] + 1 : b[0]);
    var front = fwd ? ar : al, back = fwd ? bl : br, under = fwd ? br : bl, stay = fwd ? al : ar;
    if (!front || !back) { place(to, write); return; }
    pages.forEach(function (p) { p.hidden = true; p.classList.remove("is-shown", "is-under", "is-turning"); });
    [stay, under].forEach(function (p) { if (p) { p.hidden = false; p.classList.add(p === under ? "is-under" : "is-shown"); } });
    [front, back].forEach(function (p) { p.hidden = false; p.classList.add("is-turning"); });
    // The book moves to the middle as it opens or closes.
    book.dataset.at = b[0] === 1 ? "front" : (b.length === 1 && b[0] === N) ? "back" : "";
    var sign = fwd ? -1 : 1, ease = "cubic-bezier(.42, .04, .3, 1)";
    run(to, write, function (anims) {
      anims.push(front.animate([{ transform: "rotateY(0deg)" }, { transform: "rotateY(" + 180 * sign + "deg)" }], { duration: TURN_MS, easing: ease, fill: "both" }));
      anims.push(back.animate([{ transform: "rotateY(" + -180 * sign + "deg)" }, { transform: "rotateY(0deg)" }], { duration: TURN_MS, easing: ease, fill: "both" }));
      anims.push(shade(front, [{ opacity: 0 }, { opacity: 1, offset: .5 }, { opacity: 1 }]));
      anims.push(shade(back, [{ opacity: 1 }, { opacity: 1, offset: .5 }, { opacity: 0 }]));
      if (under) anims.push(shade(under, [{ opacity: .55 }, { opacity: 1, offset: .35 }, { opacity: 0 }], TURN_MS, true));
    });
  }
  // One page open: it turns away to the left, or the one before comes back over it.
  function turnPage(to, write) {
    var fwd = to > cur, a = page(cur), b = page(to);
    var leaf = fwd ? a : b, under = fwd ? b : a;
    pages.forEach(function (p) { p.hidden = true; p.classList.remove("is-shown", "is-under", "is-turning"); });
    under.hidden = false; under.classList.add("is-under");
    leaf.hidden = false; leaf.classList.add("is-turning");
    var ease = "cubic-bezier(.42, .04, .3, 1)";
    run(to, write, function (anims) {
      anims.push(leaf.animate(fwd ? [{ transform: "rotateY(0deg)" }, { transform: "rotateY(-180deg)" }]
                                  : [{ transform: "rotateY(-180deg)" }, { transform: "rotateY(0deg)" }], { duration: TURN_MS * .8, easing: ease, fill: "both" }));
      anims.push(shade(under, fwd ? [{ opacity: .55 }, { opacity: 1, offset: .35 }, { opacity: 0 }] : [{ opacity: 0 }, { opacity: 1, offset: .65 }, { opacity: .55 }], TURN_MS * .8, true));
    });
  }
  // Moving less, or a page set to scroll: the new pages cross over the old in a moment.
  function crossFade(to, write) {
    place(to, write);
    if (typeof book.animate === "function") shown(to).forEach(function (n) {
      page(n).animate([{ opacity: 0 }, { opacity: 1 }], { duration: 160, easing: "ease-out" });
    });
  }

  /* ---------- A picture seen whole ---------- */
  // Over the book, the whole of it fitted to the screen (never enlarged),
  // its caption under it and its figure's number in the bar; a click on it,
  // or Return, shows it at its own size, from the part the page showed (or
  // the place clicked), to move round. Nothing else can be reached until
  // it's put away (Close, or Esc), and the keyboard goes back to the
  // picture it was opened from.
  var plateW = 0, plateH = 0;
  function showPlate(link) {
    var img = $("img", link);
    if (!img) return;
    plateImg.removeAttribute("srcset");
    plateImg.src = link.dataset.src || img.currentSrc || img.src;
    var set = link.dataset.srcset || img.getAttribute("srcset");
    if (set && !link.dataset.src && / \d+w(,|$)/.test(set)) set = null;
    if (set) plateImg.srcset = set;
    plateImg.alt = img.alt;
    plateW = plateImg.width = +(link.dataset.w || img.getAttribute("width"));
    plateH = plateImg.height = +(link.dataset.h || img.getAttribute("height"));
    var fig = link.closest("figure"), cap = fig && $(".cap", fig), n = fig && $(".fig-n", fig);
    plateCap.textContent = "";
    if (cap) {
      var c = cap.cloneNode(true);
      // (The page's words for its own cut, or for what only the page has, left there.)
      $$(".cap-narrow, .cap-page", c).forEach(function (e) { e.remove(); });
      while (c.firstChild) plateCap.appendChild(c.firstChild);
    }
    plateCap.hidden = !cap;
    var name = n ? n.textContent.replace(/[.\s]+$/, "") : "The Whole Picture";
    if (plateName) plateName.textContent = name;
    plate.setAttribute("aria-label", name + ", whole");
    $$(":scope > *", frame).forEach(function (e) { if (e !== plate) e.inert = true; });
    plate.hidden = false;
    plate.opener = link;
    // Where the page's cut starts (a phone's own, and as far as it was moved).
    var shown = link.closest(".fig-pic"), cs = getComputedStyle(img);
    plate.at = [Math.max(0, -parseFloat(cs.marginLeft) || 0) + (shown ? shown.scrollLeft : 0), Math.max(0, -parseFloat(cs.marginTop) || 0) + (shown ? shown.scrollTop : 0)];
    plateSize(false);
    plateClose.focus({ preventScroll: true });
  }
  // The picture fitted to what the screen leaves it under its bar, round its
  // caption (which wraps to the picture's width), or at its own size.
  function plateSize(actual) {
    var k = 1;
    plate.classList.toggle("is-actual", !!actual);
    if (!actual) {
      var bs = getComputedStyle(plateBody), cap = !plateCap.hidden;
      var aw = plateBody.clientWidth - parseFloat(bs.paddingLeft) - parseFloat(bs.paddingRight) - 2;
      var ah = plateBody.clientHeight - parseFloat(bs.paddingTop) - parseFloat(bs.paddingBottom) - 2;
      // (Twice: the caption is as tall as the width the picture leaves it.)
      k = Math.min(1, aw / plateW);
      plateCap.style.minWidth = "0";
      for (var i = 0; i < 2; i++) {
        plateCap.style.width = Math.max(1, Math.floor(plateW * k)) + "px";
        var room = ah - (cap ? plateCap.offsetHeight + parseFloat(getComputedStyle(plateCap).marginTop) : 0);
        k = Math.min(1, aw / plateW, room / plateH);
      }
      plateCap.style.width = plateCap.style.minWidth = "";
    }
    plateImg.style.width = Math.max(1, Math.floor(plateW * k)) + "px";
    plateImg.style.height = Math.max(1, Math.floor(plateH * k)) + "px";
    plate.classList.toggle("can-zoom", !actual && k < 1);
    plateBox.setAttribute("aria-label", actual ? "The picture at its own size: the arrow keys move round it" : k < 1 ? "The picture, fitted to the screen: Return shows it at its own size" : "The picture, at its own size");
    if (actual) { plateBox.scrollLeft = plate.at[0]; plateBox.scrollTop = plate.at[1]; }
  }
  function plateZoom(e) {
    var on = plate.classList.contains("is-actual");
    if (on) { plateSize(false); return; }
    if (!plate.classList.contains("can-zoom")) return;
    // (Clicked: the place clicked stays under the pointer.)
    var r = plateImg.getBoundingClientRect(), at = e ? [(e.clientX - r.left) / r.width * plateW, (e.clientY - r.top) / r.height * plateH] : null;
    plateSize(true);
    if (at) {
      var v = plateBox.getBoundingClientRect();
      plateBox.scrollLeft = at[0] - (e.clientX - v.left);
      plateBox.scrollTop = at[1] - (e.clientY - v.top);
    }
  }
  // The picture a figure opens whole: its link that shows in this look.
  function figLink(fig) {
    return $$(".fig-pic a, .plate a", fig).filter(function (a) { return a.getClientRects().length; })[0] || null;
  }
  function closePlate(refocus) {
    if (plate.hidden) return false;
    plate.hidden = true;
    plate.classList.remove("is-actual", "can-zoom");
    $$(":scope > *", frame).forEach(function (e) { e.inert = false; });
    plateImg.removeAttribute("srcset");
    plateImg.removeAttribute("src");
    var o = plate.opener;
    plate.opener = null;
    if (refocus !== false && o && document.contains(o) && o.getClientRects().length) o.focus({ preventScroll: true });
    return true;
  }

  /* ---------- Full screen ---------- */
  // (A <dialog> can't be the fullscreen element itself: the Fullscreen
  // spec rules it out. Its frame, which holds all of it, can.)
  function fsOn() { return !!document.fullscreenElement && document.fullscreenElement === frame; }
  function showFs() {
    var can = !!(document.fullscreenEnabled && typeof frame.requestFullscreen === "function"), on = fsOn();
    // (Opening, or not opened: no book to fill the screen with.)
    var none = !book || !!dlg.dataset.state;
    fsBtns.forEach(function (b) {
      b.hidden = !can;
      b.disabled = none && !on;
      b.textContent = on ? "Exit Full Screen" : "Full Screen";
      b.title = on ? "Leave full screen (Esc)" : "Fill the screen with the book";
    });
    dlg.classList.toggle("is-full", on);
  }
  function toggleFs() {
    var p = fsOn() ? document.exitFullscreen() : frame.requestFullscreen();
    if (p && p.catch) p.catch(noop);
  }
  function leaveFs() { if (fsOn()) { var p = document.exitFullscreen(); if (p && p.catch) p.catch(noop); } }
  document.addEventListener("fullscreenchange", function () {
    // (Granted only after the guide had closed: left at once.)
    if (!dlg.open && fsOn()) { leaveFs(); return; }
    showFs(); relayout(true);
  });

  /* ---------- Opening and closing ---------- */
  var HASH = /^#manual(?:\/(p|ch)-(\d+))?$/;
  // The page an address (or a number) asks for, the book being here.
  function target(where) {
    if (typeof where === "number") return clamp(where);
    var m = HASH.exec(where || "");
    if (!m || !m[1]) return want;
    if (m[1] === "p") return clamp(+m[2]);
    var t = $('.opener[data-ch="' + +m[2] + '"]', book);
    return t ? num(t) : m[2] === "0" ? 1 : contents;
  }
  // Its own page, at the place asked for: where the book can't be opened here.
  function ownPage(where) {
    var h = typeof where === "number" ? "#manual/p-" + where : HASH.test(where || "") && where !== "#manual" ? where : "";
    return SRC + h;
  }
  function agentToggle() { return $("#agent-toggle"); }
  function setAgentView(on) {
    var t = agentToggle();
    if (t && t.checked !== on) { t.checked = on; t.dispatchEvent(new Event("change")); }
  }
  function frontLabel(s) { var fr = $("#db-front"); if (fr) fr.textContent = s; }
  // A row of Agent view's tree, by what it is (where it goes, and its words)
  // and which of the rows so alike it is: the tree is made again on return,
  // perhaps without rows before it that the screen's width left out.
  // (By the element it reads, where it says so; else by its words without
  // its place, " at x,y w x h", which a change of width moves.)
  function rowKey(a) {
    if (a.dataset.of) return "of " + a.dataset.of;
    var t = "";
    $$(":scope > *", a).forEach(function (c) { if (!c.classList.contains("g")) t += c.textContent; });
    return (a.getAttribute("href") || "") + "\n" + t.replace(/\s+/g, " ").trim();
  }
  function rowsLike(tree, key) { return $$("a", tree).filter(function (x) { return rowKey(x) === key; }); }
  // Open at the page `where` names (an address, or a page number), pushing
  // an entry to go back from if `push` (the entry says it's the guide's).
  function ownLinks() { $$("[data-own]", dlg).forEach(function (a) { a.href = ownPage(asked); }); }
  // (`trigger`: the entry chosen, which the keyboard comes back to.)
  function open(where, push, trigger) {
    var id = ++req;
    asked = where;
    if (!dlg.open) {
      // (Closed a moment ago and not yet tidied after: tidied first.)
      if (wasOpen) shut();
      var t = agentToggle();
      opener = trigger || document.activeElement;
      agentWas = !!(t && t.checked);
      var tree = $("#av-tree"), key = agentWas && tree && opener && tree.contains(opener) ? rowKey(opener) : null;
      agentRow = key ? { key: key, nth: rowsLike(tree, key).indexOf(opener), top: tree.scrollTop } : null;
      setAgentView(false);
      cancelAnimationFrame(warmRaf); warmRaf = 0;
      setWarm(root.dataset.theme === "night" ? 1 : 0);
      root.classList.add("has-dialog");
      frontLabel("User’s Guide");
      // (Closed a moment ago, its entry still being gone back from: pushed
      // again once that has landed, or the landing would close it.)
      if (push && closing) repush = true;
      else if (push) history.pushState({ manual: true }, "", book ? "#manual/p-" + target(where) : HASH.test(where) ? where : "#manual");
      wasOpen = true;
      dlg.showModal();
    }
    if (book && loading) { ready(); return; }
    // The pages aren't here yet: say so, with the way to read them meanwhile.
    dlg.dataset.state = "wait";
    showFs();
    ownLinks();
    dlg.classList.remove("is-slow");
    clearTimeout(slowT);
    slowT = setTimeout(function () { dlg.classList.add("is-slow"); }, 2500);
    var w = $(".mn-wait", dlg);
    if (w) w.focus({ preventScroll: true });
    load().then(function () { if (id === req && dlg.open) ready(); },
                function () { if (id === req && dlg.open) failed(); });
  }
  function ready() {
    clearTimeout(slowT);
    dlg.classList.remove("is-slow");
    delete dlg.dataset.state;
    showFs();
    layout();
    place(target(asked));
    book.focus({ preventScroll: true });
  }
  function failed() {
    clearTimeout(slowT);
    dlg.dataset.state = "fail";
    showFs();
    // The keyboard on the default: the guide's own page, which will open.
    var r = $('.mn-alert-acts [data-for="fail"].primary', dlg);
    if (r) r.focus({ preventScroll: true });
  }
  function close() {
    if (dlg.open) dlg.close();
    if (wasOpen && !dlg.open) shut();
  }
  // (Closed by the browser, as Esc does: tidied when it says so.)
  dlg.addEventListener("close", function () { if (!dlg.open && wasOpen) shut(); });
  function shut() {
    wasOpen = false;
    req++;                                   // an open still fetching is void
    cancelLoad();
    repush = false;
    finishTurn();
    closePlate(false);
    leaveFs();
    cancelAnimationFrame(warmRaf); warmRaf = 0;
    clearTimeout(slowT);
    dlg.classList.remove("is-slow");
    delete dlg.dataset.state;
    if (!$("dialog[open]")) root.classList.remove("has-dialog");
    // The desktop as it was: Agent view again if the guide was opened from
    // it, else the window in front named in the Deskbar.
    if (agentWas) {
      setAgentView(true);
      // Opened from a row of its tree, which is made again: that row, where it was.
      var tree = $("#av-tree");
      if (agentRow && tree && (!opener || !document.contains(opener))) {
        var like = rowsLike(tree, agentRow.key), row = like[agentRow.nth] || like[0];
        tree.scrollTop = agentRow.top;
        if (row) {
          opener = row;
          // (Moved by a change of width: brought into sight.)
          var rr = row.getBoundingClientRect(), tr = tree.getBoundingClientRect();
          if (rr.top < tr.top || rr.bottom > tr.bottom) tree.scrollTop += rr.top - tr.top - (tr.height - rr.height) / 2;
        }
      }
      if (!opener || !document.contains(opener)) opener = $('#av-tree a[href="assets/manual.html"]');
    } else {
      var f = $(".sec.is-front"), a = f && $('.db-apps a[href="#' + f.id + '"]');
      frontLabel(a ? a.textContent.trim() : "");
    }
    agentWas = false;
    // Back where the page was: the guide's own entry gone back from, or
    // the address as it came, without the guide's part.
    var fromNav = navClosing;
    navClosing = false;
    if (!fromNav && HASH.test(location.hash) && !closing) {
      if (history.state && history.state.manual) { closing = true; history.back(); }
      else history.replaceState(history.state, "", location.pathname + location.search);
    }
    // (The keyboard given back to the entry is no reason to fetch it again.)
    givingBack = true;
    try {
      if (opener && opener.focus && document.contains(opener)) opener.focus({ preventScroll: true });
      // (Gone from sight meanwhile, as a link in a Deskbar that folded is: the
      // nearest control that's there takes the keyboard.)
      if (opener && document.activeElement !== opener) {
        var near = $(agentWas || document.documentElement.dataset.view === "agent" ? "#agent-back" : ".db-fold");
        if (near) near.focus({ preventScroll: true });
      }
    } finally { givingBack = false; }
    opener = null;
  }
  dlg.addEventListener("cancel", function (e) { if (closePlate()) e.preventDefault(); });
  // The address changed (Back, Forward, a link, one typed): the guide
  // follows it, without writing it. (Both events come for one change: the
  // second finds nothing to do, even while the first one's turn goes on.)
  function onNav() {
    if (closing) {
      closing = false;
      if (dlg.open) {
        // Opened again before the back landed: its entry back on top.
        if (repush) history.pushState({ manual: true }, "", book && !dlg.dataset.state ? "#manual/p-" + want : HASH.test(asked) ? asked : "#manual");
        repush = false;
        return;
      }
      if (HASH.test(location.hash)) history.replaceState(history.state, "", location.pathname + location.search);
      return;
    }
    if (HASH.test(location.hash)) {
      if (!dlg.open) open(location.hash, false);
      else if (book && !dlg.dataset.state) {
        var dest = target(location.hash);
        if (turning && sameView(dest, turning.to)) { turning.to = dest; return; }
        go(dest, false);
      } else { asked = location.hash; ownLinks(); }
    } else if (dlg.open) { navClosing = true; close(); }
  }
  window.addEventListener("popstate", onNav);
  window.addEventListener("hashchange", onNav);

  /* ---------- What the reader does ---------- */
  // Anywhere on the page: an entry to the guide (a plain click; with a
  // modifier the link opens the guide's own page, as links do).
  var ENTRY = "[data-manual], a[href^='#manual'], #av-tree a[href='assets/manual.html']";
  document.addEventListener("click", function (e) {
    var a = e.target.closest && e.target.closest(ENTRY);
    if (!a || dlg.contains(a) || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    e.preventDefault();
    var h = a.getAttribute("href");
    open(a.hasAttribute("data-manual") ? "#manual" + (a.dataset.manual ? "/" + a.dataset.manual : "") : h.charAt(0) === "#" ? h : "#manual", true, a);
  }, true);
  // Pointed at or reached by the keyboard, an entry fetches the guide ahead.
  function early(e) { if (!loading && !givingBack && e.target.closest && e.target.closest("[data-manual]")) load().catch(noop); }
  document.addEventListener("pointerover", early, { passive: true });
  document.addEventListener("focusin", early);

  dlg.addEventListener("click", function (e) {
    var t = e.target;
    if (!t.closest) return;
    var b = t.closest("[data-go]");
    if (b) { ({ next: next, prev: prev, contents: function () { go(contents); } })[b.dataset.go](); return; }
    if (t.closest("[data-close]")) { close(); return; }
    if (t.closest("[data-plate-close]")) { closePlate(); return; }
    if (t === plateImg) { plateZoom(e); return; }
    if (t.closest("[data-fullscreen]")) { toggleFs(); return; }
    if (t.closest("[data-retry]")) { open(asked, false); return; }
    if (t.closest("[data-cancel]")) { close(); return; }
    var mod = e.metaKey || e.ctrlKey || e.shiftKey || e.altKey;
    // A page or chapter in the book: turned to (with a modifier, the
    // browser's own way: a new tab, a new window).
    var a = t.closest('a[href^="#manual"]');
    if (a) { if (!mod) { e.preventDefault(); go(target(a.getAttribute("href"))); } return; }
    // The mark at a caption's end: its picture whole.
    var z = t.closest(".fig-zoom");
    if (z) { var l = !mod && figLink(z.closest("figure")); if (l) { e.preventDefault(); showPlate(l); } return; }
    // A picture cut to its point: the whole of it, over the book (a link
    // to it, opened as links are with a modifier).
    var f = t.closest(".fig-pic a, .plate a");
    if (f) { if (!mod) { e.preventDefault(); showPlate(f); } return; }
    var tab = t.closest(".mn-etab");
    if (tab) { go(+tab.dataset.page); return; }
    var c = t.closest(".mn-corner");
    if (c) { if (c.classList.contains("next")) next(); else prev(); return; }
    // The front cover opens the book.
    if (book && cur === 1 && t.closest(".page.cover") && !t.closest("a, button, input, label")) next();
  });
  // (On the document: a button that the keyboard was on and that can't be
  // pressed now leaves the keyboard with the page, outside the window.)
  document.addEventListener("keydown", function (e) {
    if (!dlg.open || e.altKey || e.ctrlKey || e.metaKey || !book || dlg.dataset.state) return;
    var t = e.target;
    if (!plate.hidden) {
      if (t === plateBox && (e.key === "Enter" || e.key === " ")) { plateZoom(null); e.preventDefault(); }
      return;
    }
    if (t.matches && t.matches("input, textarea, select")) return;
    switch (e.key) {
      case "ArrowRight": case "PageDown": next(); break;
      case "ArrowLeft": case "PageUp": prev(); break;
      case "Home": go(1); break;
      case "End": go(N); break;
      case "t": case "T": go(contents); break;
      default: return;
    }
    e.preventDefault();
  });
  // A swipe across the stage turns a page; one up or down scrolls it.
  var sw = null;
  dlg.addEventListener("pointerdown", function (e) {
    if (e.pointerType === "mouse") return;
    // A second finger, anywhere in the window: a pinch, not a swipe.
    if (!e.isPrimary || !stage.contains(e.target)) { sw = null; return; }
    // On something that scrolls sideways (a wide picture, a listing): its own.
    var x = e.target.closest && e.target.closest(".fig-pic, .listing");
    if (x && x.scrollWidth > x.clientWidth + 1) { sw = null; return; }
    sw = { id: e.pointerId, x: e.clientX, y: e.clientY, t: e.timeStamp };
  });
  stage.addEventListener("pointerup", function (e) {
    if (!sw || e.pointerId !== sw.id) return;
    var dx = e.clientX - sw.x, dy = e.clientY - sw.y, quick = e.timeStamp - sw.t < 900;
    sw = null;
    if (quick && Math.abs(dx) > 44 && Math.abs(dx) > 1.4 * Math.abs(dy)) { if (dx < 0) next(); else prev(); }
  });
  stage.addEventListener("pointercancel", function () { sw = null; });
  // The page behind stays put: only a page set to scroll, or a picture seen
  // whole, scrolls in here. (A pinch, which comes as a wheel with Control, zooms.)
  dlg.addEventListener("wheel", function (e) {
    if (e.ctrlKey) return;
    if (!e.target.closest || !e.target.closest('.manual[data-fit="flow"] .mn-stage, .mn-plate-view')) e.preventDefault();
  }, { passive: false });
  var sized = 0;
  window.addEventListener("resize", function () {
    if (!dlg.open || sized) return;
    sized = requestAnimationFrame(function () { sized = 0; relayout(false); if (!plate.hidden && !plate.classList.contains("is-actual")) plateSize(false); });
  });

  /* ---------- Night: the window warms itself ---------- */
  // A <dialog> is in the top layer, over the page's night light, so the
  // guide warms its own pixels, its pictures with the rest, as the night
  // light does the screen's: Olo blue's white point moved by a multiply of
  // #FFB46B at 80%, fading over the same 1.5 s.
  var warmM = document.getElementById("mn-warm-m"), warmNow = 0, warmRaf = 0;
  function setWarm(k) {
    warmNow = k;
    if (warmM) warmM.setAttribute("values", "1 0 0 0 0  0 " + (1 - 0.2353 * k).toFixed(4) + " 0 0 0  0 0 " + (1 - 0.4643 * k).toFixed(4) + " 0 0  0 0 0 1 0");
    frame.style.filter = k > 0 && warmM ? "url(#mn-warm)" : "";
  }
  function warmTo(goal) {
    cancelAnimationFrame(warmRaf); warmRaf = 0;
    if (goal === warmNow) return;
    if (!dlg.open || reduced.matches) { setWarm(goal); return; }
    var from = warmNow, t0 = performance.now();
    var step = function (now) {
      var t = Math.min(1, (now - t0) / 1500), e = t < .5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2;
      setWarm(from + (goal - from) * e);
      warmRaf = t < 1 ? requestAnimationFrame(step) : 0;
    };
    warmRaf = requestAnimationFrame(step);
  }
  new MutationObserver(function () {
    if (dlg.open) { warmTo(root.dataset.theme === "night" ? 1 : 0); if (book && !dlg.dataset.state) prime(shown(cur)); }
  }).observe(root, { attributes: true, attributeFilter: ["data-theme"] });

  /* ---------- Arrived at an address in the guide ---------- */
  showFs();
  if (HASH.test(location.hash)) open(location.hash, false);
})();
