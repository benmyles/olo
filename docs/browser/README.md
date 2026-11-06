# Olo Browser

This project builds Olo's own browser engine in Rust. V8 may execute
JavaScript; embedding Blink, WebKit, Servo, or a system browser as the
engine does not meet the objective. The installed browser remains
available while this one is developed. Nothing here changes defaults.

**Status: the original Rust engine now parses static HTML, resolves selected
CSS/styles and native fonts, shapes text, lays out initial normal flow and
paints bounded retained tiles in a native `olo-browser` development window.
Local HTML, progress/cancellation, initial structured visible text, scrolling
and restart restoration work. Networking, JavaScript, complete web layout,
images/media, semantic document actions, universal dark mode and Olo/password/
passkey integrations remain substantial required work. There is no demonstrated
website compatibility: 0/100 sites and 0/3,000 combinations accepted.**
The 99% target remains an acceptance criterion. Passing 100 selected sites
alone cannot establish compatibility with 99% of the web.

## Acceptance

- A native `olo-ui` browser, with tabs, navigation, history, bookmarks,
  find, downloads, file panels, Tracker reveal, printing, clipboard,
  notifications, permissions, session restoration and in-place upgrade.
- A hand-written Rust HTML/DOM/CSS/style/layout/painting engine, with V8
  bindings, event loop, modules, Fetch, storage, workers and the web APIs
  required by the corpus. Standards behavior is checked against WPT and
  independent reference implementations, not tests copied from our code.
- Structured agent snapshots and actions use the same live DOM and input
  dispatch as people. Document/frame/navigation identities prevent stale
  actions. Passwords never appear in snapshots, action logs or recordings.
- Passwords and passkeys integrate with Olo's keychain. Origin checks,
  permissions and the boundary between agent screens and the person's
  session remain enforced. Untrusted content cannot invoke desktop APIs.
- Every corpus journey passes behavioral and visual comparison with a
  pinned Chrome build, in light, dark and night, at 1, 1.25, 1.5, 1.75 and
  2×, including scale changes while open, at desktop and narrow sizes.
  Authenticated journeys use dedicated test accounts, never personal ones.
- Dark pages respect site `color-scheme`/`prefers-color-scheme`, and apply
  semantic foreground/background adaptation where the site has none.
  Images, video, canvas, gradients, controls and contrast need dedicated
  checks. A blanket inversion is insufficient.
- Release measurements compare memory, first input latency, navigation,
  layout, scrolling and GPU time with Chrome on identical workloads.
  Idle does no work. UI frames meet Olo's 8.3 ms budget. Performance claims
  include exact commands, machine/build identity and raw observations.
- Site isolation, sandboxing, network security, malicious-input resource
  limits, cancellation and recovery are required before general browsing.

## Architecture

`crates/web` owns platform-independent engine code. Stable arena handles,
source-backed text, shared interned names and bounded caches keep memory
proportional to retained content. Parsing, loading, image decoding and
layout run away from the UI thread; generation-tagged results cannot
replace a newer navigation. Work queues have bounds and backpressure.
Cancellation is checked during large operations, not just between them.

The browser shell will live in `apps/browser`, built on `olo-ui`. The
shell owns trusted desktop integration; renderer processes receive only
explicitly granted capabilities. Retained display lists feed GPU drawing;
DOM/style/layout invalidation follows changed subtrees. No polling or
per-frame reparsing, reshaping or image decoding. Scale-dependent caches
include scale, font generation and viewport in their identity.

SIMD is used for proven bulk scans (initially `memchr`'s runtime-selected
implementations), text validation and four-lane linear color conversion;
benchmarks determine where more SIMD or GPU work pays.
Scalar fallbacks and equivalent results are mandatory. GPU transfers and
additional retained memory count toward the cost, too.

BeOS references: `vendor/bebook/BMessage.md`'s typed commands and
`BView.md`'s scriptable view hierarchy inform the agent interface; the
responsive Application/Interface Kit split informs the worker boundary.
Keep Olo's shared kit conventions instead of reproducing an old browser.

## Evaluation

See [eval/README.md](eval/README.md). Missing evidence is a failure, and
fetching/parsing a homepage is never a visual or behavioral pass. The
100-site corpus is a coverage instrument, not a substitute for WPT,
security tests or a broader independently sampled compatibility study.

Primary references, consulted 2026-11-04:

- [WHATWG HTML parsing](https://html.spec.whatwg.org/multipage/parsing.html)
- [WHATWG DOM](https://dom.spec.whatwg.org/)
- [CSS specifications](https://www.w3.org/Style/CSS/specs.en.html)
- [Web Platform Tests](https://web-platform-tests.org/running-tests/)
- [CrUX methodology](https://developer.chrome.com/docs/crux/methodology)
- [CrUX dataset](https://developer.chrome.com/docs/crux/bigquery)

Progress and evidence: [TODO.md](TODO.md), [JOURNAL.md](JOURNAL.md).
Engine API, coverage gates and benchmarks: [ENGINE.md](ENGINE.md).
