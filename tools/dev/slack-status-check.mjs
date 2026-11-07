// Slack's status following Olo's, as slack-page.js does it: the page
// script run in node against a fake Slack, each web API call recorded
// and answered as Slack would, through every case (locked and back, Do
// Not Disturb for a while and off, a call, a status of the person's own
// left, ours changed by them kept, what's ours remembered).
//
//   node tools/dev/slack-status-check.mjs apps/chrome/extension/slack-page.js
import fs from "node:fs";
import vm from "node:vm";

const src = fs.readFileSync(process.argv[2], "utf8");
const calls = [];
let profile = { status_text: "", status_emoji: "" };
const store = new Map([["localConfig_v2", JSON.stringify({ teams: { T1: { id: "T1", token: "xoxc-1", url: "https://acme.slack.com/" } } })]]);
const listeners = [];
const noop = () => {};
const anything = new Proxy(function () {}, { get: (t, k) => (k === Symbol.toPrimitive ? () => "" : anything), apply: () => anything, construct: () => anything });
const window = {
  addEventListener: (t, f) => t === "message" && listeners.push(f),
  postMessage: noop,
  location: { origin: "https://app.slack.com", href: "https://app.slack.com/client/T1" },
  localStorage: { getItem: (k) => store.get(k) ?? null, setItem: (k, v) => store.set(k, String(v)) },
  fetch: async (url, init) => {
    const form = Object.fromEntries(init.body.entries());
    calls.push({ url, ...form });
    const method = url.split("/api/")[1];
    if (method === "users.profile.get") return { json: async () => ({ ok: true, profile }) };
    if (method === "users.profile.set") profile = { ...profile, ...JSON.parse(form.profile) };
    return { json: async () => ({ ok: true }) };
  },
  FormData,
  setTimeout,
  clearTimeout,
  console,
  EventTarget,
  Event,
  navigator: anything,
  ServiceWorkerRegistration: anything,
  Object, JSON, Math, Date, Map, WeakMap, Promise, URL, String, Number, Error, TypeError, Array, Symbol, Reflect,
};
window.window = window;
window.self = window;
vm.createContext(window);
vm.runInContext(src, window);
process.on("unhandledRejection", (e) => console.log("rejected", e));

const inner = vm.runInContext("window", window);
const send = (status) => listeners.forEach((f) => f({ source: inner, data: { __oloSlack: "olo", type: "status", status } }));
const settle = () => new Promise((r) => setTimeout(r, 20));
const take = () => calls.splice(0).map((c) => `${c.url.split("/api/")[1]} ${Object.entries(c).filter(([k]) => !["url", "token"].includes(k)).map(([k, v]) => `${k}=${v}`).join(" ")}`.trim());
const check = (what, got, want) => {
  const ok = JSON.stringify(got) === JSON.stringify(want);
  console.log(`${ok ? "ok  " : "FAIL"} ${what}: ${JSON.stringify(got)}`);
  if (!ok) process.exitCode = 1;
};

send({ away: false, dnd: false, dnd_until: null, call: false });
await settle();
check("nothing to do at first", take(), []);
send({ away: true, dnd: false, dnd_until: null, call: false });
await settle();
check("locked: away", take(), ["users.setPresence presence=away"]);
send({ away: false, dnd: false, dnd_until: null, call: false });
await settle();
check("unlocked: automatic", take(), ["users.setPresence presence=auto"]);
const until = Math.floor(Date.now() / 1000) + 30 * 60;
send({ away: false, dnd: true, dnd_until: until, call: false });
await settle();
check("Do Not Disturb for half an hour", take(), ["dnd.setSnooze num_minutes=30"]);
send({ away: false, dnd: true, dnd_until: until, call: false });
await settle();
check("the same again: nothing", take(), []);
send({ away: false, dnd: false, dnd_until: null, call: false });
await settle();
check("Do Not Disturb off: snooze ended", take(), ["dnd.endSnooze"]);
send({ away: false, dnd: false, dnd_until: null, call: true });
await settle();
check("a call: in a meeting", take(), ["users.profile.get", 'users.profile.set profile={"status_text":"In a meeting","status_emoji":":spiral_calendar_pad:","status_expiration":0}']);
send({ away: false, dnd: false, dnd_until: null, call: false });
await settle();
check("the call over: cleared", take(), ["users.profile.get", 'users.profile.set profile={"status_text":"","status_emoji":"","status_expiration":0}']);
// A status of the person's own: kept through a call.
profile = { status_text: "On holiday", status_emoji: ":palm_tree:" };
send({ away: false, dnd: false, dnd_until: null, call: true });
await settle();
check("their own status: left", take(), ["users.profile.get"]);
send({ away: false, dnd: false, dnd_until: null, call: false });
await settle();
check("and nothing undone", take(), []);
// Ours changed by the person during the call: kept.
profile = { status_text: "", status_emoji: "" };
send({ away: false, dnd: false, dnd_until: null, call: true });
await settle();
take();
profile = { status_text: "Lunch", status_emoji: ":sandwich:" };
send({ away: false, dnd: false, dnd_until: null, call: false });
await settle();
check("changed by them since: kept", take(), ["users.profile.get"]);
// Remembered across a reload (what's ours is in localStorage).
check("remembered", JSON.parse(store.get("oloSlackStatus")), { T1: { away: false, dnd: false, dnd_until: null, meeting: false } });
