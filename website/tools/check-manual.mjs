#!/usr/bin/env node
// The User's Guide's behaviour, checked in a headless Chrome: opening and
// failing to, history, turning, the plate, Agent view, full screen, the
// guide's own page with and without scripts, and (with --long) nothing
// running at rest across a minute's turn. It serves website/ itself, on a
// free port of 127.0.0.1, and drives Chrome over its DevTools protocol
// (Node 22 or later: its WebSocket); it opens no window anywhere.
//
//   node website/tools/check-manual.mjs [--long] [--only NAME,...]
//
// Chrome: $CHROME, else Playwright's chrome-headless-shell, else chromium,
// chromium-browser or google-chrome on the PATH. Its profile is a folder of
// its own in /var/tmp, removed once Chrome (and everything it started) has
// gone. Prints a PASS or FAIL for each check, and exits 1 if any failed (or
// the page logged an error), 2 if it was asked for a check it hasn't.
import { spawn, execFileSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const SITE = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const LONG = args.includes("--long");
const ONLY = args.includes("--only") ? (args[args.indexOf("--only") + 1] || "").split(",").filter(Boolean) : null;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---------------------------------------------------------------- the site, served
const TYPES = { ".html": "text/html; charset=utf-8", ".css": "text/css", ".js": "text/javascript", ".svg": "image/svg+xml",
  ".webp": "image/webp", ".png": "image/png", ".woff2": "font/woff2", ".md": "text/markdown; charset=utf-8", ".txt": "text/plain; charset=utf-8", ".json": "application/json" };
const server = http.createServer((q, r) => {
  let p = decodeURIComponent(new URL(q.url, "http://x").pathname);
  if (p.endsWith("/")) p += "index.html";
  const f = path.join(SITE, path.normalize(p));
  if (!f.startsWith(SITE) || !fs.existsSync(f) || fs.statSync(f).isDirectory()) { r.writeHead(404); r.end("not here"); return; }
  r.writeHead(200, { "Content-Type": TYPES[path.extname(f)] || "application/octet-stream", "Cache-Control": "no-store" });
  fs.createReadStream(f).pipe(r);
});

// ---------------------------------------------------------------- the checks
const OPENED = "document.querySelector('#manual').open && document.querySelector('#manual .book') && !document.querySelector('#manual').dataset.state";
const ST = "document.querySelector('#manual').dataset.state";
const SHOWN = "[...document.querySelectorAll('#manual .page')].filter(p => !p.hidden).map(p => +p.dataset.page).join(',')";
const PROG = "document.querySelector('#manual .mn-top .mn-progress').firstChild.textContent";
const IMGS = "[...document.querySelectorAll('main img, .deskbar img')].map(i => i.getAttribute('src') + '|' + (i.getAttribute('srcset') || '')).join(';')";
const GUIDE = "document.querySelector('.hero-actions .button.guide')";
const DLG = "document.querySelector('#manual')";
const FRONT = "document.querySelector('#db-front').textContent";
const TURNING = "document.querySelector('#manual .page.is-turning')";
const SETTLED = (s) => `${SHOWN} === '${s}' && !${TURNING}`;
const PLATE = "document.querySelector('#manual .mn-plate')";
const PIMG = "document.querySelector('#manual .mn-plate-view img')";
// The plate's picture whole inside the screen, at its own shape, its caption in sight.
const PLATE_FITS = `(() => { const i = ${PIMG}, b = document.querySelector('#manual .mn-plate-body').getBoundingClientRect(), r = i.getBoundingClientRect(), c = document.querySelector('#manual .mn-plate figcaption').getBoundingClientRect();
  const shape = Math.abs(r.width / r.height - +i.getAttribute('width') / +i.getAttribute('height')) < 0.02;
  return (!${PLATE}.hidden && r.width > 40 && r.height > 40 && i.complete && i.naturalWidth > 0 && r.left >= b.left && r.right <= b.right && r.top >= b.top && c.bottom <= b.bottom + 0.5 && shape) || [r.left, r.top, r.right, r.bottom, b.left, b.top, b.right, b.bottom, c.bottom, i.naturalWidth, shape]; })()`;
const edition = fs.readFileSync(path.join(SITE, "assets", "manual.html"), "utf8");
const TRUNCATED = edition.slice(0, edition.lastIndexOf("<section", edition.indexOf('id="manual/p-3"'))) + "</article></main></body></html>";
// (Ended right after the back cover's opening tag: 30 pages, the last a cover, and empty.)
const CUT_AT_30 = edition.slice(0, edition.indexOf(">", edition.indexOf('id="manual/p-30"')) + 1);
const PAGES_31 = edition.replace('data-pages="30"', 'data-pages="31"');
const PAGES_NONE = edition.replace(' data-pages="30"', "");
const TOO_LONG = edition.replace("</body>", "<!--" + "x".repeat(1100000) + "--></body>");
const BROKEN = "<!doctype html><title>Not the guide</title><p>A page with no book in it.</p><img src='x.png'>";
const REFUSED = (name, body, label, more = {}) => ({ name, intercept: [{ url: "*/assets/manual.html", body, uses: 1, ...more }], steps: [
  { eval: `${GUIDE}.click()` }, { until: `${ST} === 'fail'`, timeout: 8000 },
  { check: "!document.querySelector('#manual .book') || 'a book was kept'", label },
  { clickOn: "#manual [data-retry]" }, { until: OPENED, timeout: 8000 },
  { check: "document.querySelectorAll('#manual .page').length === 30 || document.querySelectorAll('#manual .page').length", label: "and Try Again opens all 30 pages" } ] });
// A row of Agent view's tree for an element: where the element is in the page (as site.js says it).
const PATH_OF = "(el => { const p = []; for (let e = el; e && e !== document.body; e = e.parentElement) { if (e.id) { p.unshift('#' + e.id); break; } let i = 0, s = e; while ((s = s.previousElementSibling)) i++; p.unshift(i); } return p.join('>'); })";

const cases = [
  { name: "malformed-then-retry", intercept: [{ url: "*/assets/manual.html", body: BROKEN, uses: 1 }], steps: [
    { eval: `window.__imgs = ${IMGS}` }, { eval: `${GUIDE}.click()` }, { until: `${ST} === 'fail'`, timeout: 6000 },
    { check: `${IMGS} === window.__imgs || 'the page\\'s pictures changed'`, label: "an answer with no book in it leaves the homepage alone" },
    { check: "!document.querySelector('#manual .book') || 'built'", label: "no book is built from it" },
    { check: "document.activeElement === document.querySelector('#manual .mn-alert-acts [data-for=\"fail\"].primary[data-own]') || document.activeElement.outerHTML.slice(0, 80)", label: "the failure takes the keyboard to its default, the guide's own page" },
    { check: "document.querySelector('#manual .mn-top [data-fullscreen]').disabled || 'Full Screen live'", label: "Full Screen can't be chosen with no book" },
    { clickOn: "#manual [data-retry]" }, { until: OPENED, timeout: 6000 },
    { check: `${SHOWN} === '1' || ${SHOWN}`, label: "Try Again opens the book at its cover" },
    { check: "!document.querySelector('#manual .mn-top [data-fullscreen]').disabled || 'still off'", label: "and Full Screen can be chosen again" } ] },
  { name: "truncated-edition", intercept: [{ url: "*/assets/manual.html", body: TRUNCATED, uses: 1 }], steps: [
    { eval: `${GUIDE}.click()` }, { until: `${ST} === 'fail'`, timeout: 6000 },
    { check: "!document.querySelector('#manual .book') || 'a book of two pages was kept'", label: "a whole HTTP answer with only part of the guide is refused" },
    { clickOn: "#manual [data-retry]" }, { until: OPENED, timeout: 6000 },
    { check: "document.querySelectorAll('#manual .page').length === 30 || document.querySelectorAll('#manual .page').length", label: "and the next attempt opens all 30 pages" } ] },
  REFUSED("edition-cut-at-back-cover", CUT_AT_30, "an answer cut off at the back cover's start (30 pages, the last an empty cover) is refused"),
  REFUSED("edition-pages-31", PAGES_31, "an edition whose data-pages says 31 for 30 pages is refused"),
  REFUSED("edition-pages-missing", PAGES_NONE, "an edition that doesn't say how many pages it has is refused"),
  REFUSED("edition-too-long", TOO_LONG, "an answer past 1 MB (no Content-Length: read in pieces) is refused"),
  { name: "cached-failure", cache: true, intercept: [{ url: "*/assets/manual.html", body: BROKEN, uses: 1, cacheable: true }], steps: [
    { eval: `${GUIDE}.click()` }, { until: `${ST} === 'fail'`, timeout: 6000 },
    { clickOn: "#manual [data-retry]" }, { until: `${OPENED} || ${ST} === 'fail'`, timeout: 8000 },
    { check: `(${OPENED}) || 'Try Again got the cached failure'`, label: "Try Again asks the server, not a cache holding the failure" } ] },
  { name: "css-fails-then-retry", intercept: [{ url: "*/assets/manual.css", status: 404, body: "gone", type: "text/plain", uses: 1 }], expect: ["manual.css"], steps: [
    { eval: `${GUIDE}.click()` }, { until: `${ST} === 'fail'`, timeout: 6000 },
    { check: "!document.querySelector('link[data-manual-css]') || 'still there'", label: "a stylesheet that failed is taken out" },
    { clickOn: "#manual [data-retry]" }, { until: OPENED, timeout: 6000 },
    { check: "!!document.querySelector('link[data-manual-css]').dataset.loaded", label: "Try Again loads it" } ] },
  { name: "css-stalls", intercept: [{ url: "*/assets/manual.css", hold: true, uses: 1 }], expect: ["manual.css"], steps: [
    { eval: `${GUIDE}.click()` }, { until: `${ST} === 'wait'`, timeout: 3000 },
    { until: `${ST} === 'fail'`, timeout: 24000 },
    { check: "!document.querySelector('link[data-manual-css]') || 'the stalled link is still there'", label: "a stylesheet that never comes fails the attempt at its deadline, its link taken out" },
    { clickOn: "#manual [data-retry]" }, { until: OPENED, timeout: 6000 },
    { check: "true", label: "and Try Again opens the book" } ] },
  { name: "prefetch-fails-then-click", intercept: [{ url: "*/assets/manual.css", status: 404, body: "gone", type: "text/plain", uses: 1 }], expect: ["manual.css"], steps: [
    { eval: `${GUIDE}.dispatchEvent(new PointerEvent('pointerover', { bubbles: true }))` }, { wait: 800 },
    { eval: `${GUIDE}.click()` }, { until: OPENED, timeout: 6000 },
    { check: `!${ST} || ${ST}`, label: "a failed prefetch doesn't stick: choosing the entry fetches again" } ] },
  { name: "close-while-loading", intercept: [{ url: "*/assets/manual.html", delay: 1500, pass: true }], steps: [
    { eval: `${GUIDE}.click()` }, { until: `${ST} === 'wait'`, timeout: 3000 },
    { check: "document.querySelector('#manual .mn-top [data-fullscreen]').disabled || 'Full Screen live'", label: "while it opens, Full Screen can't be chosen" },
    { clickOn: "#manual [data-cancel]" }, { wait: 2200 },
    { check: `!${DLG}.open && !document.querySelector('#manual .book') || 'built after Cancel'`, label: "Cancel while opening drops the attempt: nothing is built after (the keyboard given back fetches nothing)" } ] },
  { name: "fallback-follows-address", intercept: [{ url: "*/assets/manual.html", delay: 4000, pass: true }], url: "#manual/p-3", steps: [
    { until: `${ST} === 'wait'`, timeout: 3000 }, { eval: "location.hash = '#manual/p-17'" }, { wait: 2800 },
    { check: "[...document.querySelectorAll('#manual [data-own]')].every(a => /manual\\.html#manual\\/p-17$/.test(a.href)) || document.querySelector('#manual [data-own]').href", label: "the way to the guide's own page follows the address while it opens" },
    { check: "getComputedStyle(document.querySelector('#manual .mn-alert-acts .mn-wait-f')).visibility === 'visible' || 'hidden'", label: "slow, it offers the guide as a web page, a button beside Cancel" } ] },
  { name: "p-99", url: "#manual/p-99", steps: [
    { until: OPENED, timeout: 6000 },
    { check: `${SHOWN} === '30' && location.hash === '#manual/p-30' || [${SHOWN}, location.hash]`, label: "a page past the end opens the last, and says so" } ] },
  { name: "back-while-fetching", intercept: [{ url: "*/assets/manual.html", delay: 1800, pass: true }], steps: [
    { clickOn: ".hero-actions .button.guide" }, { until: `${ST} === 'wait'`, timeout: 3000 }, { eval: "history.back()" },
    { until: `!${DLG}.open`, timeout: 3000 }, { wait: 2600 },
    { check: `!${DLG}.open && location.hash === '' || [${DLG}.open, location.hash]`, label: "Back while fetching closes it, and it stays closed" } ] },
  // Back while a page is still turning (its animation held, so it is): the
  // turn lands without writing its page over the entry Back went to.
  { name: "back-during-turn", url: "#manual/p-6", steps: [
    { until: OPENED, timeout: 6000 }, { eval: "location.hash = '#manual/p-12'" }, { until: SETTLED("12,13"), timeout: 3000 },
    { key: "ArrowRight" }, { until: TURNING, timeout: 600 },
    { eval: `if (!${TURNING}) throw new Error('not turning'); document.getAnimations().forEach(a => a.pause()); history.back();` },
    { until: "location.hash === '#manual/p-6' || location.hash === '#manual/p-14'", timeout: 2000 },
    { eval: "document.getAnimations().forEach(a => a.play())" }, { until: SETTLED("6,7"), timeout: 3000 },
    { check: `${SHOWN} === '6,7' && location.hash === '#manual/p-6' || [${SHOWN}, location.hash]`, label: "Back during a turn lands where Back went, the address unchanged" } ] },
  // Back to an address of a chapter: the page turns (the second event for
  // the change doesn't cut its turn short), and the address stays the chapter's.
  { name: "back-to-chapter", url: "#manual/p-12", steps: [
    { until: OPENED, timeout: 6000 }, { eval: "location.hash = '#manual/ch-1'" }, { until: SETTLED("6,7"), timeout: 3000 },
    { eval: "location.hash = '#manual/p-12'" }, { until: SETTLED("12,13"), timeout: 3000 },
    { eval: "history.back()" }, { until: TURNING, timeout: 600 },
    { check: `!!${TURNING} || 'no turn'`, label: "Back turns the page, its turn not cut short by the hashchange after it" },
    { until: SETTLED("6,7"), timeout: 3000 },
    { check: `location.hash === '#manual/ch-1' || location.hash`, label: "and leaves the address as Back found it (#manual/ch-1)" } ] },
  // Opened again before the Back of its close has landed: from the close
  // event itself, and in the same moment as the close (open, and opening).
  { name: "reopen-from-close-event", steps: [
    { eval: `${GUIDE}.click()` }, { until: OPENED, timeout: 6000 },
    { eval: `${DLG}.addEventListener('close', () => ${GUIDE}.click(), { once: true }); document.querySelector('#manual [data-close]').click();` }, { wait: 1000 },
    { check: `${DLG}.open && ${SHOWN} === '1' && /^#manual/.test(location.hash) && !!(history.state && history.state.manual) && ${FRONT} === 'User’s Guide' || [${DLG}.open, ${SHOWN}, location.hash, JSON.stringify(history.state), ${FRONT}]`, label: "opened again from the close event, it stays open on an entry of its own" },
    { clickOn: "#manual [data-close]" }, { wait: 800 },
    { check: `!${DLG}.open && location.hash === '' && ${FRONT} !== 'User’s Guide' || [${DLG}.open, location.hash, ${FRONT}]`, label: "and closes cleanly after" } ] },
  { name: "reopen-at-once", steps: [
    { eval: `${GUIDE}.click()` }, { until: OPENED, timeout: 6000 },
    { eval: `document.querySelector('#manual [data-close]').click(); const a = document.createElement('a'); a.href = '#manual/p-12'; a.id = '__to12'; document.body.append(a); a.click();` }, { wait: 1200 },
    { check: `${DLG}.open && ${SHOWN} === '12,13' && location.hash === '#manual/p-12' && !!(history.state && history.state.manual) && ${FRONT} === 'User’s Guide' || [${DLG}.open, ${SHOWN}, location.hash, JSON.stringify(history.state), ${FRONT}]`, label: "closed and opened at another page in one moment, it shows that page, on an entry of its own" },
    { clickOn: "#manual [data-close]" }, { wait: 800 },
    { check: `!${DLG}.open && location.hash === '' || [${DLG}.open, location.hash]`, label: "and closes cleanly after" } ] },
  { name: "reopen-at-once-while-opening", intercept: [{ url: "*/assets/manual.html", delay: 1500, pass: true }], steps: [
    { eval: `${GUIDE}.click()` }, { until: `${ST} === 'wait'`, timeout: 3000 },
    { eval: `document.querySelector('#manual [data-cancel]').click(); ${GUIDE}.click();` },
    { check: `${DLG}.open && ${ST} === 'wait' || [${DLG}.open, ${ST}]`, label: "cancelled and chosen again in one moment, it says it's opening" },
    { until: `${OPENED} || ${ST} === 'fail'`, timeout: 8000 },
    { check: `(${OPENED}) && ${FRONT} === 'User’s Guide' || [${ST}, ${FRONT}]`, label: "and opens (the cancelled attempt doesn't fail the new one)" } ] },
  { name: "reload-then-close", steps: [
    { clickOn: ".hero-actions .button.guide" }, { until: OPENED, timeout: 6000 }, { key: "ArrowRight" }, { wait: 900 }, { key: "ArrowRight" }, { wait: 900 },
    { reload: true }, { until: OPENED, timeout: 6000 },
    { check: `${SHOWN} === '4,5' || ${SHOWN}`, label: "after a reload it opens where it was" },
    { clickOn: "#manual [data-close]" }, { wait: 1200 },
    { check: `!${DLG}.open && location.hash === '' && location.host === new URL(document.baseURI).host || location.href`, label: "Close after a reload stays on the page" } ] },
  { name: "deep-link-close", url: "about:blank", steps: [
    { nav: "#manual/p-7" }, { until: OPENED, timeout: 6000 }, { clickOn: "#manual [data-close]" }, { wait: 800 },
    { check: "location.hash === '' || location.href", label: "Close on a deep link drops the hash and stays" },
    { eval: "location.hash = '#manual/p-7'" }, { until: OPENED, timeout: 6000 }, { eval: "location.hash = '#apps'" },
    { until: `!${DLG}.open`, timeout: 3000 }, { eval: "history.back()" }, { until: OPENED, timeout: 6000 },
    { clickOn: "#manual [data-close]" }, { wait: 1200 },
    { check: "location.protocol === 'http:' || location.href", label: "and never leaves the site" } ] },
  { name: "rapid-turns", url: "#manual/p-7", steps: [
    { until: OPENED, timeout: 6000 }, { key: "ArrowRight" }, { key: "ArrowLeft" }, { key: "ArrowRight" }, { key: "ArrowRight" }, { wait: 1600 },
    { check: `${SHOWN} === '10,11' && location.hash === '#manual/p-10' && ${PROG} === 'Pages 10–11 of 30' || [${SHOWN}, location.hash]`, label: "→ ← → → from 6–7 lands on 10–11" },
    { check: `!document.querySelector('#manual .page.is-turning, #manual .page.is-under') || 'left turning'`, label: "no page left mid-turn" } ] },
  { name: "bar-subtitles", url: "#manual/p-1", reducedMotion: true, steps: [
    { until: OPENED, timeout: 6000 },
    { check: "document.querySelector('#manual .mn-top .mn-progress small').textContent === 'User’s Guide to Olo' || document.querySelector('#manual .mn-top .mn-progress').textContent", label: "the front cover's bar names the book under it" },
    { eval: "location.hash = '#manual/p-28'" }, { wait: 300 },
    { check: "document.querySelector('#manual .mn-top .mn-progress small').textContent === 'Chapter 6: Quick Reference · Index' || document.querySelector('#manual .mn-top .mn-progress').textContent", label: "28–29: Quick Reference, then the Index" } ] },
  { name: "logical-page", url: "#manual/p-7", reducedMotion: true, steps: [
    { until: OPENED, timeout: 6000 }, { viewport: [700, 900] }, { wait: 500 },
    { check: `${SHOWN} === '7' || ${SHOWN}`, label: "made narrow, p-7 shows page 7 alone" }, { viewport: [1440, 900] }, { wait: 500 },
    { eval: "location.hash = '#manual/p-29'" }, { wait: 900 },
    { check: "document.querySelector('#manual .mn-etab.ix').getAttribute('aria-current') === 'true'", label: "on the index's page its tab is current" } ] },
  { name: "single-page-turn", w: 700, h: 900, url: "#manual/p-1", steps: [
    { until: OPENED, timeout: 6000 }, { wait: 300 }, { key: "ArrowRight" }, { until: TURNING, timeout: 600 },
    { check: "/^linear-gradient\\(to right/.test(getComputedStyle(document.querySelector('#manual .page.verso .turn-shade.under')).backgroundImage) || getComputedStyle(document.querySelector('#manual .page.verso .turn-shade.under')).backgroundImage.slice(0, 40)", label: "one page alone: the shadow on the page under the leaf falls from its left edge, the hinge" } ] },
  // A Ctrl-click (trusted) on a page in the Contents: a new tab at that
  // page's address, and the book left where it was.
  { name: "modified-click", url: "#manual/p-3", reducedMotion: true, steps: [
    { until: OPENED, timeout: 6000 },
    { clickOn: "#manual .page[data-page='3'] a[href='#manual/p-11']", modifiers: 2 },
    { newTab: "*/#manual/p-11", label: "Ctrl-click on a page in the Contents opens its address in a new tab", timeout: 3000 },
    { check: `${SHOWN} === '2,3' || ${SHOWN}`, label: "and leaves the book where it is" } ] },
  { name: "plate-keyboard", w: 640, h: 760, url: "?theme=light#manual/p-13", reducedMotion: true, steps: [
    { until: OPENED, timeout: 6000 }, { clickOn: "#manual .page[data-page='13'] .fig-zoom.only-light" }, { until: `${PIMG}.complete && ${PIMG}.naturalWidth > 0`, timeout: 3000 }, { wait: 200 },
    { check: `!${PLATE}.hidden && document.activeElement === document.querySelector('#manual [data-plate-close]') || document.activeElement.outerHTML.slice(0, 80)`, label: "the mark at a caption's end opens the picture whole, Close holding the keyboard" },
    { check: `${PIMG}.getAttribute('src') === document.querySelector('#manual .page[data-page="13"] .fig-pic a.only-light img').currentSrc || ${PIMG}.getAttribute('src')`, label: "the picture it opens is the figure's" },
    { check: "document.querySelector('#manual .mn-plate-bar .mn-name span').textContent === 'Figure 2-3' || document.querySelector('#manual .mn-plate-bar .mn-name span').textContent", label: "the bar names it by its figure's number" },
    { check: PLATE_FITS, label: "the whole picture fits the screen, at its own shape, its caption under it" },
    { key: "Tab" }, { check: "document.activeElement === document.querySelector('#manual .mn-plate-view') || document.activeElement.outerHTML.slice(0, 80)", label: "Tab reaches the picture's view" },
    { key: "Enter" }, { wait: 200 },
    { check: `${PLATE}.classList.contains('is-actual') && ${PIMG}.getBoundingClientRect().width === +${PIMG}.getAttribute('width') || [${PLATE}.className, ${PIMG}.getBoundingClientRect().width, ${PIMG}.getAttribute('width')]`, label: "Return shows it at its own size" },
    { key: "ArrowRight" }, { key: "ArrowRight" }, { key: "ArrowRight" }, { wait: 300 },
    { check: "document.querySelector('#manual .mn-plate-view').scrollLeft > 0 || 'did not move'", label: "the arrow keys move round a picture wider than the screen" },
    { check: "[...document.querySelectorAll('#manual .mn-frame > *')].filter(e => !e.classList.contains('mn-plate')).every(e => e.inert)", label: "everything behind it is inert" },
    { key: "Escape" }, { wait: 300 },
    { check: `${PLATE}.hidden && ${DLG}.open && document.activeElement.matches('.fig-pic a') || document.activeElement.outerHTML.slice(0, 80)`, label: "Escape puts it away, the keyboard back on the picture" } ] },
  { name: "plate-desktop-1-1", w: 1280, h: 800, url: "?theme=dark#manual/p-7", reducedMotion: true, steps: [
    { until: OPENED, timeout: 6000 }, { clickOn: "#manual .page[data-page='7'] .fig-zoom.only-dark" }, { until: `${PIMG}.complete && ${PIMG}.naturalWidth > 0`, timeout: 4000 }, { wait: 200 },
    { check: PLATE_FITS, label: "the desktop seen whole fits the screen, at its own shape (Deskbar and Trash in it)" },
    { check: "!/callouts/.test(document.querySelector('#manual .mn-plate figcaption').textContent) || document.querySelector('#manual .mn-plate figcaption').textContent", label: "its caption doesn't promise the page's numbered callouts" } ] },
  { name: "plate-phone", w: 390, h: 844, dpr: 2, mobile: true, url: "?theme=light#manual/p-11", reducedMotion: true, steps: [
    { until: OPENED, timeout: 6000 }, { eval: "document.querySelector('#manual .page[data-page=\"11\"] .fig-pic').scrollIntoView({ block: 'center' })" }, { wait: 300 },
    { clickOn: "#manual .page[data-page='11'] .fig-pic a.only-light" }, { until: `${PIMG}.complete && ${PIMG}.naturalWidth > 0`, timeout: 3000 }, { wait: 200 },
    { check: `(() => { const v = document.querySelector('#manual .mn-plate-view').getBoundingClientRect(); return (!${PLATE}.hidden && v.width > 40 && v.height > 40 && v.left >= 0 && v.right <= innerWidth) || [${PLATE}.hidden, v.left, v.right, v.width, v.height]; })()`, label: "on a phone the picture's view is there, inside the screen" },
    { check: PLATE_FITS, label: "and the whole picture fits it, cut nowhere" },
    { check: "(() => { const c = document.querySelector('#manual .mn-plate figcaption').getBoundingClientRect(); return (c.width > 0 && c.left >= 0 && c.right <= innerWidth) || [c.left, c.right]; })()", label: "its caption in view" } ] },
  // A swipe turns a page on a phone; two fingers (the second on the bar,
  // which the page never sees) don't, nor does one across a listing that
  // scrolls sideways.
  { name: "swipes", w: 390, h: 844, dpr: 2, mobile: true, url: "?theme=light#manual/p-11", reducedMotion: true, steps: [
    { until: OPENED, timeout: 6000 }, { wait: 300 },
    { swipe: { on: "#manual .page[data-page='11'] .pg-body > p", dx: -170 } }, { wait: 400 },
    { check: `${SHOWN} === '12' || ${SHOWN}`, label: "a swipe to the left turns to the next page" },
    { swipe: { on: "#manual .page[data-page='12'] .pg-body > p", dx: -170, second: "#manual .mn-top .mn-name" } }, { wait: 400 },
    { check: `${SHOWN} === '12' || ${SHOWN}`, label: "with a second finger down on the bar meanwhile, it doesn't" },
    { eval: "location.hash = '#manual/p-16'" }, { wait: 400 }, { eval: "document.querySelector('#manual .page[data-page=\"16\"] .listing').scrollIntoView({ block: 'center' })" }, { wait: 300 },
    { check: "(l => l.scrollWidth > l.clientWidth + 1 || 'the listing fits: nothing to test')(document.querySelector('#manual .page[data-page=\"16\"] .listing'))", label: "(page 16's listing scrolls sideways on a phone)" },
    { swipe: { on: "#manual .page[data-page='16'] .listing", dx: -170 } }, { wait: 400 },
    { check: `${SHOWN} === '16' || ${SHOWN}`, label: "nor does one across a listing that scrolls sideways" } ] },
  { name: "looks", url: "?theme=dark#manual/p-23", reducedMotion: true, steps: [
    { until: OPENED, timeout: 6000 },
    { check: "(i => i.length === 6 && i.every(x => x.classList.contains('only-dark')) || i.map(x => x.className))([...document.querySelectorAll('#manual .page[data-page=\"23\"] .looks-m .tabpic img')].filter(x => x.getClientRects().length))", label: "dark: all six looks' tabs in Figure 5-1 are their dark ones" },
    { eval: "document.querySelector('#manual .page[data-page=\"23\"] .looks-m input[value=\"aqua\"]').click()" }, { wait: 100 },
    { check: "(z => z.length === 1 && /look-aqua-dark-1to1-2x\\.webp$/.test(z[0].href) || z.map(x => x.href))([...document.querySelectorAll('#manual .page[data-page=\"23\"] .fig-zoom')].filter(x => x.getClientRects().length))", label: "Aqua chosen, Figure 5-2's mark opens Aqua's picture" } ] },
  { name: "default-pressed", url: "?theme=dark", steps: [
    { wait: 300 }, { press: ".hero-actions .button.primary" }, { wait: 600 },
    { check: "getComputedStyle(document.querySelector('.hero-actions .button.primary')).boxShadow.includes('1px 1px 3px') || getComputedStyle(document.querySelector('.hero-actions .button.primary')).boxShadow", label: "dark: the default button pressed shows its pressed bevel" }, { release: true } ] },
  { name: "agent-view", steps: [
    { eval: "document.querySelector('#agent-toggle').click()" }, { until: "document.documentElement.dataset.view === 'agent'", timeout: 3000 },
    { eval: "document.querySelector('#deskbar').classList.contains('is-folded') && document.querySelector('.db-fold').click()" }, { wait: 200 },
    { clickOn: "#deskbar .db-guide" }, { until: OPENED, timeout: 6000 }, { clickOn: "#manual [data-close]" }, { wait: 800 },
    { check: "document.documentElement.dataset.view === 'agent' && document.activeElement.matches('.db-guide, #agent-back, .db-fold') || [document.documentElement.dataset.view, document.activeElement.outerHTML.slice(0, 60)]", label: "opened from Deskbar in Agent view, it closes back to Agent view, the keyboard on Deskbar's link (or, Deskbar folding, on its fold button)" },
    { clickOn: "#av-tree a[href='assets/manual.html']" }, { until: OPENED, timeout: 6000 }, { clickOn: "#manual [data-close]" }, { wait: 800 },
    { check: "document.activeElement.matches(\"#av-tree a[href='assets/manual.html']\") || document.activeElement.outerHTML.slice(0, 60)", label: "opened from its tree, the keyboard comes back to that row" },
    { check: "(t => { const r = t.getBoundingClientRect(), f = document.querySelector('.av-tools').getBoundingClientRect(), lh = parseFloat(getComputedStyle(t).lineHeight), pt = parseFloat(getComputedStyle(t).paddingTop); const whole = Math.abs(((t.clientHeight - pt) / lh) - Math.round((t.clientHeight - pt) / lh)) < 0.01; return (Math.abs(r.bottom - f.top) < 1.5 && whole) || [r.bottom, f.top, (t.clientHeight - pt) / lh]; })(document.querySelector('#av-tree'))", label: "its tree (and scroll bar) runs down to the bar at the window's foot, whole lines in it" },
    { key: "Escape" }, { wait: 300 }, { check: "document.documentElement.dataset.view !== 'agent'", label: "Escape leaves Agent view" } ] },
  { name: "agent-row-narrowed", steps: [
    { eval: "document.querySelector('#agent-toggle').click()" }, { until: "document.documentElement.dataset.view === 'agent'", timeout: 3000 },
    { eval: `window.__of = ${PATH_OF}(${GUIDE}); const r = document.querySelector('#av-tree a[data-of="' + window.__of + '"]'); if (!r) throw new Error('no row for the hero\\'s guide button'); r.id = '__row'; r.scrollIntoView({ block: 'center' });` }, { wait: 100 },
    { clickOn: "#__row" }, { until: OPENED, timeout: 6000 }, { viewport: [1100, 800] }, { wait: 400 },
    { clickOn: "#manual [data-close]" }, { wait: 900 },
    { check: "document.activeElement.closest('#av-tree') && document.activeElement.dataset.of === window.__of || [document.activeElement.outerHTML.slice(0, 80), window.__of]", label: "opened from a row below rows a narrower window leaves out, closed narrower, the keyboard comes back to that row" } ] },
  { name: "agent-escape-folded", w: 1100, h: 800, steps: [
    { eval: "document.querySelector('#agent-toggle').click()" }, { until: "document.documentElement.dataset.view === 'agent'", timeout: 3000 },
    { check: "document.querySelector('#deskbar').classList.contains('is-folded') || 'not folded'", label: "(Deskbar folded at 1100)" },
    { key: "Escape" }, { wait: 300 },
    { check: "document.documentElement.dataset.view !== 'agent' && document.activeElement === document.querySelector('.db-fold') || document.activeElement.outerHTML.slice(0, 60)", label: "Escape leaves Agent view, the keyboard on Deskbar's fold button (not lost to the page)" } ] },
  { name: "standalone-noscript", w: 390, h: 844, dpr: 2, mobile: true, noScript: true, url: "assets/manual.html", steps: [
    { check: "document.getElementById('book').dataset.fit === 'flow' && document.documentElement.scrollWidth <= 390 || [document.getElementById('book').dataset.fit, document.documentElement.scrollWidth]", label: "the guide's own page reads on a phone without scripts" },
    { check: "getComputedStyle(document.querySelector('.mp-book')).display === 'none' || 'shown'", label: "without scripts it doesn't offer Read as a Book" },
    { check: "(p => getComputedStyle(p.querySelector('.needs-js')).display === 'none' && getComputedStyle(p.querySelector('.no-js')).display !== 'none' && getComputedStyle(p.querySelector('.only-book')).display === 'none')(document.querySelector('.page[data-page=\"24\"]')) || 'wrong words'", label: "page 24 points to Olo's own Night Mode, not a link only a script makes work" } ] },
  { name: "standalone-scripted", w: 1280, h: 800, url: "assets/manual.html?theme=light", steps: [
    { wait: 300 },
    { check: "getComputedStyle(document.querySelector('.mp-book')).display !== 'none' && (p => getComputedStyle(p.querySelector('.needs-js')).display !== 'none' && getComputedStyle(p.querySelector('.no-js')).display === 'none')(document.querySelector('.page[data-page=\"24\"]')) || 'wrong'", label: "with scripts it offers Read as a Book, and page 24 its night link" },
    { check: "(() => { const mid = (e) => { const r = e.getBoundingClientRect(); return r.top + r.height / 2; }, n = document.querySelector('.mp-links'); return (n.getBoundingClientRect().height < 40 && Math.abs(mid(n) - mid(document.querySelector('.mp-name'))) < 2) || [n.getBoundingClientRect().height, mid(n), mid(document.querySelector('.mp-name'))]; })()", label: "its header on one row at 1280" },
    { eval: "document.querySelector('.page[data-page=\"23\"] .looks-m input[value=\"tokyo-night\"]').click()" }, { wait: 100 },
    { check: "(z => z.length === 1 && /look-tokyo-night-1to1-2x\\.webp$/.test(z[0].href) || z.map(x => x.href))([...document.querySelectorAll('.page[data-page=\"23\"] .fig-zoom')].filter(x => x.getClientRects().length))", label: "Tokyo Night chosen, Figure 5-2's link is to Tokyo Night's picture (no script needed)" } ] },
  { name: "standalone-532", w: 532, h: 900, url: "assets/manual.html", steps: [
    { wait: 300 }, { check: "(() => { const b = document.getElementById('book'), p = b.querySelector('.page'); return b.dataset.fit === 'fixed' && p.getBoundingClientRect().width >= 500 || [b.dataset.fit, p.getBoundingClientRect().width]; })()", label: "printed pages on its own page from a column of 532 points (a page of 500)" } ] },
  { name: "standalone-547", w: 547, h: 900, url: "assets/manual.html", steps: [
    { wait: 300 }, { check: "(() => { const b = document.getElementById('book'), p = b.querySelector('.page'); return b.dataset.fit === 'fixed' && p.getBoundingClientRect().width >= 500 || [b.dataset.fit, p.getBoundingClientRect().width]; })()", label: "at 547, printed pages of 500 points or more" } ] },
  { name: "standalone-531", w: 531, h: 900, url: "assets/manual.html", steps: [
    { wait: 300 }, { check: "document.getElementById('book').dataset.fit === 'flow' || document.getElementById('book').dataset.fit", label: "at 531, set to read (a page would be under 500)" } ] },
  { name: "standalone-scroll-bar", w: 548, h: 900, url: "assets/manual.html", steps: [
    { wait: 300 }, { eval: "document.body.style.marginRight = '20px'" }, { wait: 300 },
    { check: "(() => { const b = document.getElementById('book'), p = b.querySelector('.page'); return b.dataset.fit === 'flow' || [b.dataset.fit, p.getBoundingClientRect().width]; })()", label: "548 wide with a 20-point scroll bar's room taken: set to read, never a printed page under 500" } ] },
  { name: "fullscreen", url: "#manual/p-12", steps: [
    { until: OPENED, timeout: 6000 }, { clickOn: "#manual .mn-top [data-fullscreen]" },
    { until: "document.fullscreenElement === document.querySelector('#manual .mn-frame')", timeout: 3000 }, { wait: 250 },
    { check: "document.querySelector('#manual .mn-top [data-fullscreen]').textContent === 'Exit Full Screen'", label: "Full Screen fills the screen with the book" },
    { clickOn: "#manual .mn-top [data-fullscreen]" }, { until: "!document.fullscreenElement && document.querySelector('#manual .mn-top [data-fullscreen]').textContent === 'Full Screen'", timeout: 3000 },
    { check: `!document.fullscreenElement && ${DLG}.open && document.querySelector('#manual .mn-top [data-fullscreen]').textContent === 'Full Screen' || document.querySelector('#manual .mn-top [data-fullscreen]').textContent`, label: "Exit Full Screen leaves it, the book still open" },
    { clickOn: "#manual .mn-top [data-fullscreen]" }, { until: "document.fullscreenElement", timeout: 3000 },
    { clickOn: "#manual [data-close]" }, { until: "!document.fullscreenElement", timeout: 3000 },
    { check: `!document.fullscreenElement && !${DLG}.open`, label: "Close leaves full screen too" } ] },
  { name: "fullscreen-close-at-once", url: "#manual/p-12", steps: [
    { until: OPENED, timeout: 6000 },
    { eval: "document.querySelector('#manual .mn-top [data-fullscreen]').click(); document.querySelector('#manual [data-close]').click();" }, { wait: 1200 },
    { check: `!document.fullscreenElement && !${DLG}.open || [String(document.fullscreenElement && document.fullscreenElement.className), ${DLG}.open]`, label: "closed while full screen is still being asked for, the closed guide never fills the screen" } ] },
  { name: "deskbar-fold", w: 1100, h: 800, steps: [
    { eval: "document.querySelector('.db-fold').click()" }, { clickOn: ".hero-actions .button.guide" }, { until: OPENED, timeout: 6000 },
    { key: "Escape" }, { wait: 500 },
    { check: "!document.querySelector('#manual').open && document.querySelector('#deskbar').dataset.folded === 'false' || document.querySelector('#deskbar').dataset.folded", label: "Escape closes the guide and leaves Deskbar as it was" } ] },
  { name: "wheel", url: "#manual/p-12", steps: [
    { until: OPENED, timeout: 6000 },
    { check: "(() => { const s = document.querySelector('#manual .mn-stage'); const a = new WheelEvent('wheel', { deltaY: 40, ctrlKey: true, bubbles: true, cancelable: true }); s.dispatchEvent(a); const b = new WheelEvent('wheel', { deltaY: 40, bubbles: true, cancelable: true }); s.dispatchEvent(b); return (!a.defaultPrevented && b.defaultPrevented) || [a.defaultPrevented, b.defaultPrevented]; })()", label: "a pinch passes, a scroll of the page behind doesn't" } ] },
];
// At rest across a minute's turn: no timer or frame callback run, no style
// worked out, no layout, under a millisecond of script (its own question
// included). Timers are counted by wrapping them before the page's scripts run.
const COUNT_TIMERS = `(() => { window.__fired = 0; for (const n of ["setTimeout", "setInterval", "requestAnimationFrame", "requestIdleCallback"]) {
  const f = window[n]; if (typeof f !== "function") continue;
  window[n] = function (cb, ...rest) { if (typeof cb !== "function") return f.call(window, cb, ...rest); return f.call(window, function (...a) { window.__fired++; return cb.apply(this, a); }, ...rest); }; } })();`;
const IDLE = { name: "idle-minute", url: "#manual/p-12", countTimers: true, steps: [{ until: OPENED, timeout: 6000 }, { wait: 2500 }, { idle: 64000, label: "65 s open across a minute boundary: nothing runs" }] };
if (LONG) cases.push(IDLE);

// What was asked for is there (a long check only with --long).
if (ONLY) {
  const known = new Set([...cases.map((c) => c.name), IDLE.name]);
  const bad = ONLY.filter((n) => !known.has(n));
  if (bad.length || !ONLY.length) { console.error(bad.length ? `no such check: ${bad.join(", ")}` : "--only names no check"); process.exit(2); }
  if (ONLY.includes(IDLE.name) && !LONG) { console.error("idle-minute runs only with --long"); process.exit(2); }
}

// ---------------------------------------------------------------- Chrome
function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  const pw = path.join(os.homedir(), ".cache", "ms-playwright");
  if (fs.existsSync(pw)) {
    for (const d of fs.readdirSync(pw).filter((d) => d.startsWith("chromium_headless_shell")).sort().reverse()) {
      const f = path.join(pw, d, "chrome-headless-shell-linux64", "chrome-headless-shell");
      if (fs.existsSync(f)) return f;
    }
  }
  for (const c of ["chromium", "chromium-browser", "google-chrome"]) {
    try { return execFileSync("sh", ["-c", `command -v ${c}`]).toString().trim(); } catch {}
  }
  console.error("no Chrome: set CHROME to a Chrome, Chromium or chrome-headless-shell");
  process.exit(2);
}
const CHROME = findChrome();
// (On disk, never /tmp: that is memory.)
let profile;
try { profile = fs.mkdtempSync("/var/tmp/olo-check-manual-"); }
catch (e) { console.error(`can't make Chrome's profile in /var/tmp: ${e.message}`); process.exit(2); }
await new Promise((ok) => server.listen(0, "127.0.0.1", ok));
const BASE = `http://127.0.0.1:${server.address().port}/`;
const env = { ...process.env };
delete env.WAYLAND_DISPLAY; delete env.DISPLAY;
// (In a process group of its own: stopping it stops everything it started.)
const chrome = spawn(CHROME, ["--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check",
  "--hide-scrollbars", "--force-color-profile=srgb", "--disable-extensions", "--mute-audio", "about:blank"], { env, stdio: ["ignore", "ignore", "pipe"], detached: true });
chrome.stderr.resume();
let quitting = false;
// Chrome and its children stopped, and only once Chrome has gone, its
// profile removed (never under a Chrome still running).
function quit(code) {
  if (quitting) return;
  quitting = true;
  server.close();
  const gone = () => { try { fs.rmSync(profile, { recursive: true, force: true }); } catch {} process.exit(code); };
  if (chrome.exitCode !== null || chrome.signalCode !== null) { try { process.kill(-chrome.pid, "SIGKILL"); } catch {} return gone(); }
  chrome.once("exit", () => setTimeout(gone, 200));
  setTimeout(() => { console.error(`Chrome didn't stop: its profile is left at ${profile}`); process.exit(code || 1); }, 5000).unref();
  try { process.kill(-chrome.pid, "SIGKILL"); } catch { try { chrome.kill("SIGKILL"); } catch {} }
}
for (const sig of ["SIGINT", "SIGTERM", "SIGHUP"]) process.on(sig, () => quit(130));
process.on("uncaughtException", (e) => { console.error(e.stack || e); quit(1); });
process.on("unhandledRejection", (e) => { console.error((e && e.stack) || e); quit(1); });
setTimeout(() => { console.error("gave up after 12 minutes"); quit(124); }, 720000).unref();
chrome.on("exit", (c) => { if (!quitting) { console.error("chrome exited: " + c); quit(1); } });
// Its port, when it writes it (looked for every 100 ms, the event loop free between).
let port = "";
for (const t0 = Date.now(); !port; await sleep(100)) {
  if (Date.now() - t0 > 15000) { console.error("chrome did not start"); quit(1); await new Promise(() => {}); }
  try { port = fs.readFileSync(path.join(profile, "DevToolsActivePort"), "utf8").split("\n")[0].trim(); } catch {}
}
const ws = new WebSocket((await (await fetch(`http://127.0.0.1:${port}/json/version`)).json()).webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener("open", r, { once: true }));
let id = 0;
const pending = new Map(), listeners = [], tabs = [];
ws.addEventListener("message", (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { const p = pending.get(m.id); pending.delete(m.id); m.error ? p.no(new Error(JSON.stringify(m.error))) : p.ok(m.result); }
  else {
    if ((m.method === "Target.targetCreated" || m.method === "Target.targetInfoChanged") && m.params.targetInfo.type === "page") tabs.push(m.params.targetInfo);
    listeners.forEach((f) => f(m));
  }
});
const send = (method, params = {}, sessionId) => new Promise((ok, no) => { const i = ++id; pending.set(i, { ok, no }); ws.send(JSON.stringify({ id: i, method, params, sessionId })); });
const globRe = (g) => new RegExp("^" + g.split("*").map((x) => x.replace(/[.+?^${}()|[\]\\]/g, "\\$&")).join(".*") + "$");
await send("Target.setDiscoverTargets", { discover: true });

// ---------------------------------------------------------------- running them
const results = [], problems = [];
for (const c of cases) {
  if (ONLY && !ONLY.includes(c.name)) continue;
  const { targetId } = await send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await send("Target.attachToTarget", { targetId, flatten: true });
  const S = (m, p) => send(m, p, sessionId);
  tabs.length = 0;
  const seen = (m) => {
    if (m.sessionId !== sessionId) return;
    const quiet = (t) => (c.expect || []).some((x) => t.includes(x));
    if (m.method === "Runtime.exceptionThrown") problems.push(`[${c.name}] exception: ${m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text}`);
    if (m.method === "Log.entryAdded" && m.params.entry.level === "error" && !quiet(m.params.entry.text + (m.params.entry.url || ""))) problems.push(`[${c.name}] ${m.params.entry.text} ${m.params.entry.url || ""}`);
    if (m.method === "Fetch.requestPaused") {
      const rule = (c.intercept || []).find((r) => globRe(r.url).test(m.params.request.url));
      (async () => {
        if (!rule) return S("Fetch.continueRequest", { requestId: m.params.requestId });
        if (rule.uses !== undefined && rule.uses-- <= 0) return S("Fetch.continueRequest", { requestId: m.params.requestId });
        if (rule.hold) return;
        if (rule.delay) await sleep(rule.delay);
        if (rule.pass) return S("Fetch.continueRequest", { requestId: m.params.requestId });
        return S("Fetch.fulfillRequest", { requestId: m.params.requestId, responseCode: rule.status || 200,
          responseHeaders: [{ name: "Content-Type", value: rule.type || "text/html; charset=utf-8" }, { name: "Cache-Control", value: rule.cacheable ? "max-age=300" : "no-store" }],
          body: Buffer.from(rule.body || "").toString("base64") });
      })().catch(() => {});
    }
  };
  listeners.push(seen);
  await S("Runtime.enable"); await S("Log.enable"); await S("Network.enable"); await S("Page.enable");
  if (!c.cache) await S("Network.setCacheDisabled", { cacheDisabled: true });
  if (c.intercept) await S("Fetch.enable", { patterns: c.intercept.map((r) => ({ urlPattern: r.url })) });
  if (c.countTimers) await S("Page.addScriptToEvaluateOnNewDocument", { source: COUNT_TIMERS });
  const W = c.w || 1440, H = c.h || 900;
  await S("Emulation.setDeviceMetricsOverride", { width: W, height: H, deviceScaleFactor: c.dpr || 1, mobile: !!c.mobile, screenWidth: W, screenHeight: H });
  if (c.mobile) await S("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
  if (c.reducedMotion) await S("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "reduce" }] });
  if (c.noScript) await S("Emulation.setScriptExecutionDisabled", { value: true });
  const nav = async (url, reload) => {
    const loaded = new Promise((r) => { const f = (m) => { if (m.sessionId === sessionId && m.method === "Page.loadEventFired") { listeners.splice(listeners.indexOf(f), 1); r(); } }; listeners.push(f); });
    if (reload) await S("Page.reload", {}); else await S("Page.navigate", { url });
    await loaded;
    if (!c.noScript) await S("Runtime.evaluate", { expression: "document.fonts.ready.then(() => 1)", awaitPromise: true });
  };
  const at = (u) => (u === "about:blank" ? u : BASE + (u || ""));
  await nav(at(c.url));
  const ev = async (expr) => {
    const r = await S("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true, userGesture: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
    return r.result.value;
  };
  const centre = (sel) => ev(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return null; const r = e.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2]; })()`);
  const touch = (type, points) => S("Input.dispatchTouchEvent", { type, touchPoints: points.map(([x, y, i]) => ({ x, y, id: i })) });
  for (const st of c.steps) {
    try {
      if (st.wait) await sleep(st.wait);
      if (st.viewport) await S("Emulation.setDeviceMetricsOverride", { width: st.viewport[0], height: st.viewport[1], deviceScaleFactor: c.dpr || 1, mobile: !!c.mobile, screenWidth: st.viewport[0], screenHeight: st.viewport[1] });
      if (st.nav) await nav(at(st.nav));
      if (st.reload) await nav(null, true);
      if (st.eval) await ev(st.eval);
      if (st.until) {
        let ok = false;
        for (const t0 = Date.now(); Date.now() - t0 < (st.timeout || 5000);) { if (await ev(`!!(${st.until})`)) { ok = true; break; } await sleep(20); }
        if (!ok) problems.push(`[${c.name}] timed out waiting for: ${st.until}`);
      }
      if (st.key) {
        const vk = { Escape: 27, Tab: 9, Enter: 13, ArrowRight: 39, ArrowLeft: 37, Home: 36, End: 35 }[st.key] || 0;
        const text = st.key === "Enter" ? "\r" : undefined;
        await S("Input.dispatchKeyEvent", { type: "keyDown", key: st.key, code: st.key, windowsVirtualKeyCode: vk, text });
        await S("Input.dispatchKeyEvent", { type: "keyUp", key: st.key, code: st.key, windowsVirtualKeyCode: vk });
      }
      if (st.clickOn) {
        const xy = await centre(st.clickOn);
        if (!xy) problems.push(`[${c.name}] nothing to click: ${st.clickOn}`);
        else for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) await S("Input.dispatchMouseEvent", { type, x: xy[0], y: xy[1], button: "left", clickCount: 1, modifiers: st.modifiers || 0 });
      }
      if (st.swipe) {
        // One finger across, from the middle of `on`; with `second`, another
        // finger down on that and up again first (so far as the page knows,
        // only if it hears of it outside the stage).
        const a = await centre(st.swipe.on), b = st.swipe.second ? await centre(st.swipe.second) : null;
        if (!a || (st.swipe.second && !b)) problems.push(`[${c.name}] nothing to swipe: ${st.swipe.on}`);
        else {
          await touch("touchStart", [[a[0], a[1], 1]]);
          if (b) { await touch("touchStart", [[a[0], a[1], 1], [b[0], b[1], 2]]); await sleep(30); await touch("touchMove", [[a[0], a[1], 1]]); }
          for (const k of [0.33, 0.66, 1]) { await sleep(25); await touch("touchMove", [[a[0] + st.swipe.dx * k, a[1], 1]]); }
          await touch("touchEnd", []);
        }
      }
      // The mouse held down on something (it is :active), and let go.
      if (st.press) {
        const xy = await centre(st.press);
        if (!xy) problems.push(`[${c.name}] nothing to press: ${st.press}`);
        else { c.pressed = xy; for (const type of ["mouseMoved", "mousePressed"]) await S("Input.dispatchMouseEvent", { type, x: xy[0], y: xy[1], button: "left", clickCount: 1 }); }
      }
      if (st.release && c.pressed) await S("Input.dispatchMouseEvent", { type: "mouseReleased", x: c.pressed[0], y: c.pressed[1], button: "left", clickCount: 1 });
      if (st.newTab) {
        const re = globRe(st.newTab);
        let hit = null;
        for (const t0 = Date.now(); !hit && Date.now() - t0 < (st.timeout || 3000); await sleep(50)) hit = tabs.find((t) => t.targetId !== targetId && re.test(t.url));
        results.push(`${hit ? "PASS" : "FAIL"} [${c.name}] ${st.label}${hit ? "" : " :: " + JSON.stringify(tabs.filter((t) => t.targetId !== targetId).map((t) => t.url))}`);
        for (const t of new Set(tabs.filter((t) => t.targetId !== targetId).map((t) => t.targetId))) await send("Target.closeTarget", { targetId: t }).catch(() => {});
        tabs.length = 0;
      }
      if (st.check) {
        const v = await ev(st.check);
        results.push(`${v === true ? "PASS" : "FAIL"} [${c.name}] ${st.label}${v === true ? "" : " :: " + JSON.stringify(v)}`);
      }
      if (st.idle) {
        await S("Performance.enable");
        const NEED = ["ScriptDuration", "LayoutCount", "RecalcStyleCount"];
        const read = async () => {
          const m = (await S("Performance.getMetrics")).metrics, o = {};
          for (const k of NEED) { const x = m.find((y) => y.name === k); if (!x) throw new Error(`Chrome gave no ${k}`); o[k] = x.value; }
          o.fired = await ev("window.__fired");
          if (typeof o.fired !== "number") throw new Error("the timers weren't counted");
          return o;
        };
        const a = await read();
        await sleep(st.idle);
        const b = await read();
        const script = (b.ScriptDuration - a.ScriptDuration) * 1000, layouts = b.LayoutCount - a.LayoutCount, styles = b.RecalcStyleCount - a.RecalcStyleCount, timers = b.fired - a.fired;
        const ok = timers === 0 && styles === 0 && layouts === 0 && script < 1;
        results.push(`${ok ? "PASS" : "FAIL"} [${c.name}] ${st.label} (timers and frames +${timers}, style recalcs +${styles}, layouts +${layouts}, script +${script.toFixed(2)} ms)`);
      }
    } catch (e) {
      problems.push(`[${c.name}] ${JSON.stringify(st).slice(0, 100)}: ${e.message}`);
    }
  }
  listeners.splice(listeners.indexOf(seen), 1);
  await send("Target.closeTarget", { targetId });
}
console.log(results.join("\n"));
const failed = results.filter((r) => r.startsWith("FAIL")).length;
if (problems.length) console.log("PROBLEMS:\n" + problems.join("\n"));
console.log(failed || problems.length ? `${failed} of ${results.length} checks failed` : `all ${results.length} checks passed`);
ws.close();
quit(failed || problems.length ? 1 : 0);
