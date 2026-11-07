# Hacking on Olo

Olo is a Wayland desktop environment in Rust that captures the look and
feel of BeOS R5, with deliberate modern polish. **North star: everything is
pixel-perfect and beautiful, in light, dark and night modes, at 1×, 1.5×
and 2× (and any other scale).** Every deviation from BeOS should be an intentional
improvement, never an accident.

**Wayland only, no X11.** app_server is a Wayland compositor and nothing
else: there is no Xwayland, no X11 window manager and no `DISPLAY`, and
X11-only programs aren't supported. Don't add X11 code, features or
dependencies; programs that can do either (Chrome, GTK, Qt) are started
as Wayland clients (e.g. Chrome's `--ozone-platform=wayland`).

## The responsiveness rule (applies to everything)

**Nothing may ever block the UI.** BeOS was famous for staying responsive
under load; Olo must be at least as good. This is a hard requirement for
every app, the kit, app_server and the shell, and it is checked in review
like pixel alignment is.

1. **The UI thread only draws and reacts.** Anything that can take more than
   a frame (~8 ms), or that *might* (disk, network, subprocesses, D-Bus,
   large folders, image decoding, hashing, searching, sysfs on a slow
   device), runs on a worker thread and reports back through a `Proxy`
   (`olo_ui::proxy(cx)`, taken in `message_handler`) or a channel a GPUI
   task waits on. No `std::fs` on folders of unknown size, no blocking IPC
   round-trips and no `wait()` on the UI thread. Nor the clipboard: the
   program that copied writes it when it gets round to it (or never, if
   it's stuck), so Paste reads it with `cx.paste_item()` (or
   `paste_as(types)` for one form) and puts it in when it comes, and a
   Paste item is enabled by `cx.clipboard_has_text()`, not a read; never
   `read_from_clipboard`.
2. **Show progress and keep it live.** Long work shows what it is doing:
   counts, the current item, a `StatusBar` for work with a known size, and
   a "Searching…"/"Copying…" state otherwise. Results stream in as they
   arrive instead of appearing all at once at the end.
3. **Everything long can be cancelled** (Stop button, Escape, closing the
   window), and cancelling takes effect immediately (workers check an
   `AtomicBool`).
4. **The interface reacts in the same frame** as the input that caused it:
   pressed states, selection, menus and text never wait for work to finish.
   When an action changes state, `cx.notify()` the view that shows it.
5. **Batch updates** from workers (every ~100–150 ms) so streaming results
   never flood the event loop, and look at shared progress counters on a
   timer only while work is running.
6. **app_server never waits on a client.** Clients that stall must not stall
   the desktop. Nor on the GPU driver: a client's new buffer is checked
   (its first import, milliseconds on some GPUs, every frame of a resize)
   on a worker (`dmabuf_check`), and a commit showing it waits for that
   rather than importing it on the loop. The worker checks on a GPU device
   of its own (Mesa locks per device, and finishing a frame takes the
   renderer's) and holds each buffer till the renderer has let it go, so
   the kernel's freeing of old buffers (1–4 ms a window-sized one, done in
   the next bind into a GPU address space) happens on the worker, never in
   the renderer's bind under its lock; a few at a time (`FREE_AT_ONCE`),
   so a check coming in waits behind two, not a dozen (45 ms, a drag's
   round held back with it). A bind of the renderer's own big
   buffers' leftovers (a screenshot's) still lands in its next import:
   keep app_server's own big GPU buffers few and long-lived. Measure with
   `tools/dev/perf-fuzz` (`{"type":"timings"}` names what held the loop;
   a dispatch of clients' requests names the request in it that took
   most of it: a commit by its role, a new popup, a popup's grab, as
   `Timings::within` notes them).
7. **Stay current without polling.** What is on screen reflects the system
   as it changes: follow files and folders with `olo_ui::watch` (inotify:
   one thread per app, asleep until the kernel has an event; a burst
   gathered for 25 ms and read whole at its end; a path followed whether
   it is a file or a folder, exists yet or not, is replaced by a rename or
   swapped for another folder; the items that changed in a folder come
   with it, so a caller passes over what isn't its business), Tracker's
   own `watch.rs` (changed items stat'ed off the UI thread, a full relist
   on queue overflow, a folder renamed or swapped away watched again at
   its path, the nearest folder above watched for it to come back if
   nothing is there yet, and the folders above each one watched for
   being moved or deleted, which takes it from its path without a word
   to its own watch), and the mount table with `POLLPRI` on
   `/proc/self/mountinfo`. No timers that re-read the disk to look for
   changes; keep busy files (frames, caches) out of a folder others
   follow. What follows them: Tracker's windows, desktop, Trash, Get Info
   and Find results; the file panels; StyledEdit's and Viewer's open
   files (another program's change is shown at once unless there are
   unsaved changes; told from our own save by its time and length) and
   Viewer's Previous / Next; the palette's file index and applications;
   Deskbar's applications, notification count, agent screens and
   upgrade; Agent Screens' list; Preferences ▸ Notifications; Logs' Live
   journal (journald touches the file for readers after each write).
   Follow app_server with `Client::into_events_of(kinds)`, asking only
   for the kinds used (`EventKind`): `Windows` is every window whenever
   anything about any changes, an agent's spinning title ten times a
   second, so only what shows windows asks for it (Deskbar); `OwnWindows`
   is the program's own, only when where they are changes (olo-ui's
   `workspaces::follow`), and `Focus` the focused one as the focus moves (the
   supervisor). `Painted` says a display drew something new (headless
   screens; only to whoever names it, as it comes with frames): Agent
   Screens' Live takes a picture then rather than on a timer.
   app_server writes each event out once for everyone.
8. **app_server never deadlocks.** A hang there freezes the whole desktop.
   Smithay's per-surface lock isn't re-entrant: inside a closure given to
   `with_pending_state`, `with_states`, `with_committed_state` or
   `with_cached_state`, call nothing that takes it again
   (`WindowElement::decorated`/`inner_size`/`outer_size`/`content_offset`/
   `title`/`app_id`/`geometry`, sending a configure, mapping the element).
   Work such values out before the call, and never nest these calls,
   not even on two surfaces (a subsurface and its root: Smithay's
   lock-order checker aborts in debug builds, inside a Wayland handler
   that can't unwind, as `new_fractional_scale` did for a Firefox
   window in Focus). `tests/surface_locks.rs` checks the source for
   this, method calls and bare calls (`with_states(…)` inside another)
   alike; `tools/dev/wm-sweep` exercises every window
   operation on several screens and fails on the first hang. Nor does
   anything else in Olo: every lock is `olo_sync`'s (crates/sync:
   `Mutex`, `RwLock`, `Condvar`, never `std::sync`'s, Smithay's own
   included), which in debug builds checks the order locks are taken
   in: a lock taken again by the thread holding it, or two taken in
   both orders anywhere (the deadlock waiting to happen, though this run
   never interleaved them), panics at once with both stacks (run with
   `RUST_LIB_BACKTRACE=1`), so tests, test sessions, the fuzzers on
   debug builds (`olo-fuzz sweep --debug`) and `wm-sweep` find it
   before anyone hangs; release builds get std's
   locks as they are. (It found Smithay's keyboard taking its keymap's
   lock and its state's in both orders.) `tools/dev/lockcheck` looks
   without running: clippy's lints for guards held through a `match`
   or past their last use, `lockcheck lockbud`, a static analysis
   of double locks and conflicting orders across functions and crates,
   and `lockcheck reentry`, GPUI's double lock: an entity read or
   updated while it's being updated ("The GPUI kit").
   Hold a lock for as little as you can: take what you need, let go,
   then call out; never hold one across a call back into app_server or
   anything that might take another lock.
9. **Every frame within 8.3 ms: 120 frames a second, always.** Olo's
   displays run at 120 Hz, and a frame that takes longer than one
   skips. That covers a whole frame (the app's code, GPUI's layout and
   paint, and the GPU) on release builds, filling a 16" MacBook Pro's
   screen, at 1×, 1.5× and 2×. Nothing per frame touches the disk or
   waits on anyone (an icon is decoded off the thread; a desktop entry
   looked up once, on a worker); what a frame lays out is what shows,
   not all there is (a list's rows, `uniform_list`; the editor's
   paragraphs; History's cards). Nor does it set the GPU up: a swapchain
   made is new GPU memory, 10 ms and more for a big one on Apple's GPU,
   so a window resized by hand gets one as big as its screen on the
   press and shows only its part (`wp_viewport`), made to fit 1.5 s
   after it ends; windows and menus share the device's pipelines and
   glyph atlas, so one opens without building them; glyphs a window will
   show are drawn ahead on a thread (gpui/README.md). A frame runs on a
   fast core at a high clock: GPUI's main thread asks the scheduler for
   it as it starts (its `uclamp.min`, `olo_protocol::performance::
   UI_UTIL_MIN`, 768 of 1024; threads it starts afterwards don't
   inherit it), app_server's main thread too (`CORE_UTIL_MIN`, 1024).
   Otherwise a thread that's mostly idle is put on a slow core at a low
   clock, and the frame after a pause, the one that matters, took three
   or four times as long there (Activity's first frame of its
   processes: 13–17 ms, 3.7–4.6 with it; text shaped 6× faster). It
   costs nothing while the thread sleeps. `OLO_UTIL_MIN=N` (0 for none)
   to compare. Pictures are
   decoded and resampled on workers (`ImageView`, `pictures::thumbnail`,
   `icons`), the frame only drawing them. `tools/dev/bench` checks every
   app (see Testing).

   A display with nothing new rests (`render::IDLE_FRAMES`, both
   backends): after 3 looks in a row that found no damage it looks
   only every 100 ms (`IDLE_POLL`), and at once when woken
   (`Backend::wake`, paced a quarter of a frame after its last look):
   a commit wakes the displays its window is on (every display for a
   popup, a layer or the pointer; none for a window showing nowhere),
   and so do input (pointer motion and scrolling only the displays under
   the pointer, before and after: the first time, every motion woke
   every display, up to four looks a frame each with a DRM test commit,
   and the desktop stuttered), a request that changes something (not `ping`,
   `list_windows` and the other questions), the windows or workspaces
   changing (`tick`), a capture asking for a frame, and app_server's
   own overlays (the level indicator, keyboard hints, the lock screen,
   a wallpaper arriving; a display doesn't rest while one goes up a
   strip a frame). Each look gathers every element and compares them
   (on a real display, a DRM test commit too): 120 a second for each of
   two displays kept an idle app_server at a fifth of a core. Something
   new of ours that shows without a wake shows at the next look, up to
   100 ms late: wake for it (at debug level the log says "a resting
   display found something new at its look"). The keep-warm pixel
   alone doesn't wake a resting display. `OLO_REST=off` looks every
   frame, as before, and `{"type":"set_resting","on":false}` at run
   time. Woken looks come no sooner than a frame after the last, aimed at
   the display's next vblank. Nothing rests while something of
   app_server's own moves (`keeps_awake`: a scroll being paced or its
   glide, a window dragged, the overview, Twitcher, hints, the spotlight,
   the level indicator, a glow fading): a frame of it may change nothing
   to see, and a glide paced by looks went on at ten frames a second.
   `{"type":"timings","quiet":true}` answers with `phases`: what the
   loop spent its time on (each look's parts, `tick`'s, commits, input).

   A frame's budget is the frame less what the GPU then takes to draw
   it: app_server shows a commit once its GPU work is done (the dmabuf
   blocker), so a window drawing in 6 ms whose GPU took 2 more missed
   every other frame. app_server tells clients to draw as it starts
   drawing the screen, not once it has (`send_frames` in `pre_repaint`),
   so they have the whole frame.

   What looks the same frame after frame isn't made again: a view that
   didn't change is replayed from its last paint (cached views,
   `part::Part`: "The GPUI kit"), a row of text keeps its glyphs placed
   (`GlyphRun`: Terminal's rows, the editor's), and work a frame can't do
   in its budget is spread over frames (the editor measures paragraphs
   ahead a millisecond a frame; formatted documents lay out what shows
   first, the rest 3 ms a frame after).

## Fast and light (applies to everything)

**Olo is fast and lightweight: a core tenet, held as strictly as
pixel-perfection.** Every CPU cycle spent has to buy something someone
sees or uses; work that changes nothing is a bug. It is checked in
review, and a change isn't done until it has been measured.

What it means in numbers, on release builds:

* **Idle** (nothing on screen changing): app_server and every app at
  about 0% of a core, asleep. app_server's displays rest (about ten
  looks a second each, see rule 9); an app wakes only for input, an
  event it asked for or work it was given. No polling: no timers that
  look for changes (rule 7), no frames when nothing changed (the bench
  checks an idle second draws nothing), no events subscribed to that
  aren't used (`into_events_of`).
* **A little changing** (an agent's spinner, a blinking cursor, a
  clock): the cost follows what changed, not what's on screen: a
  spinner's row, not the terminal. The whole desktop stays in single
  digits of one core with an agent working in a full-screen Terminal
  (Terminal: 1.2 ms a frame whole, 0.1 ms of it for the grid). A widget
  whose look follows the pointer (lit when hovered, pushed when
  pressed) has the view it's painted in drawn again: `let view =
  window.current_view()` as it paints, `cx.notify(view)` in its
  handler, as GPUI's own hover styles do. Not `window.refresh()`,
  which renders every view of the window again, cached or not: the
  kit's buttons, sidebars, dashboard cards, slim scroll bars and tab
  strips did, and a pointer swept over Notifications' history (a
  screen of 200 cards) took frames of 9 to 46 ms, its others 2 ms
  (now 3.1 to 3.3 ms at worst; refresh is for what changes the whole
  window: its size, its scale, its look).
* **A lot changing** (scrolling, a live resize, video): every frame
  within 8.3 ms (rule 9), drawn by the GPU, and no more CPU than that
  takes.
* **Over time**: memory, threads and file descriptors don't grow
  (`tools/dev/perf-fuzz` checks).

How to measure (before and after, the same case, numbers in the commit
message):

* `tools/dev/pw-sandbox DIR start|env|stop`: a sound server of its
  own (PipeWire, its PulseAudio server, WirePlumber) in DIR under
  /var/tmp, with no hardware, nothing from the network, no D-Bus and its
  own state: made-up speakers ("speakers") and microphone ("mic", which
  hears what's played into "talk"). `eval "$(tools/dev/pw-sandbox DIR
  env)"` and run anything that plays, records or changes the defaults
  there, never in the person's (the voice service makes itself the
  default input). A tsession app reaches it with
  `PULSE_SERVER=unix:DIR/run/pulse/native`.
* `tools/dev/cost PID [SECONDS]`: a running process's CPU per thread
  (user and kernel), wakeups and preemptions a second, and for a kit
  app each window's frames and their cost by phase (ui, tessellating,
  submitting). Read-only, so it works on a test session's processes
  and, when the person asks, on theirs.
* app_server: `tsession ipc NAME '{"type":"timings"}'` twice, seconds
  apart: each display's looks, their total and slowest. Asking starts a
  4 ms heartbeat for a minute, so measure its CPU before asking or a
  minute after.
* `tools/dev/bench` (every app's frames against the budget; `--env
  K=V` gives every app a switch, to compare without a rebuild, as
  `--env OLO_PAINT_MEMO=0`),
  `tools/dev/scroll-check` (smoothness and the first frame's latency;
  `--trace` for each frame), `tools/dev/perf-fuzz` (stalls, slow
  answers, growth).
* Where a GPUI app's frame goes: `OLO_FRAME_PHASES=1` prints each
  frame's layout, paint, scene finish and present (which waits for the
  previous frame's GPU work), how many sprites, quads and paint
  operations it has, and the lines it shaped and glyphs it drew afresh.
  A frame much slower than the same work warm may have run on a slow
  core (rule 9: compare `OLO_UTIL_MIN=0`). Where its time goes inside: sample its UI thread
  with `eu-stack -1 -i -p PID` a few hundred times while the bench's
  pointer sweeps it and count the functions on the stacks (perf isn't
  installed).
* GPU time: a GPUI app run with `OLO_GPU_TIMINGS=1` prints each frame's
  passes as the GPU timed them, its quads (in screens filled: more than
  one is overdraw) and its sprites. A frame shows on time only if its
  CPU *and* GPU work are done before app_server draws the screen, which
  shares the GPU: headless app_server logs `frame skipped: the GPU is
  still drawing the last one` when it couldn't.
* Measure the case the change is for (idle; a spinner; scrolling a big
  folder; a window full screen on a 16" MacBook Pro's 3456×2234 at 2×)
  on release builds, compare with the build before, and check the
  latency too: a saving that makes the first frame after input later
  isn't one. A guess ("should be cheap") isn't a measurement.

How Olo keeps it (do the same):

* **Nothing changed, nothing done.** app_server's displays rest and are
  woken by what can change them (rule 9); a window draws only when a
  view in it is notified or asks for an animation frame, and one that
  doesn't show gets no frame callbacks. Don't notify on a timer to look
  for changes.
* **Work done once is kept**, keyed by what it was made from:
  Terminal's rows (`RowCache`: each row's cells compared with those it
  was drawn from, only changed rows laid out and tessellated again),
  `recorded::Recorded`, `Cell::text_cached`, window tab looks, the
  wallpaper's strips. Compare the source (cells, a version counter)
  rather than hashing what you'd draw: finding out what changed must
  cost far less than drawing it.
* **Only what shows, only what changed**: virtualised lists, per-row
  and per-item caches, damage.
* **Ask for less**: events by kind (`EventKind`: `OwnWindows`, not every
  window's title), files by inotify, an event written out once for all.
  Something that wakes every app costs every app: an agent's spinning
  title woke every app's two kit threads ten times a second.
* **Per frame, no waste**: no new strings or vectors for what didn't
  change, nothing quadratic (a check of a whole run per character cost
  Terminal 1 ms a frame), no locks held across calls (Deskbar's status
  deadlocked holding one), no sleeping loops.
* **The GPU draws, the CPU decides.** Upload once and keep textures;
  meshes made once and handed over as they are.

## Crash reports (crates/crash, applies to every program)

**Every Olo program leaves what's needed to look into its crash.**
`olo_crash::install()` is the first thing each program's `main` does
(`olo_ui::run` does it again, harmlessly; `crates/crash/tests/
every_program.rs` fails for a `main` in apps/ or crates/ that doesn't
start with it). It has nothing but libc, so app_server, the session,
brush and the root Wi-Fi healer carry it as lightly as the kit's apps.

- **A report**, one JSON file a crash, in `olo_crash::dir()`
  (`~/.local/state/olo/crashes/PROGRAM-PID-MILLIS.json`; a service
  with no home, as root, /var/lib/olo/crashes), written beside as
  `.NAME` and renamed, so a reader never sees half of one; the newest
  ten of each program kept, four at most from one process. Every
  report has `program`, `pid`, `exe`, `build` (the executable's time),
  `started_ms`, `at_ms`, `kind` and `thread`, and:
  - a panic (any thread's, the program going on or not: a worker's
    panic is reported though the window stays): `message`, `place`
    (file:line:column) and `backtrace` (release builds keep line
    tables, so it names files and lines), from a panic hook that then
    calls std's (its message on stderr as ever);
  - a fatal signal (SIGSEGV, SIGBUS, SIGILL, SIGFPE, SIGABRT): `signal`,
    `code`, and the fault's `address` or, for one sent (kill), `sent_by`;
    `core` names `coredumpctl info PID`. The handler is
    async-signal-safe (system calls into buffers on its stack, the
    folders made one by one with mkdir, nothing allocated or locked),
    kept out of line so the handler's own frame is small (a signal can
    come on what's left of a thread's alternate stack: std's
    stack-overflow abort does). Then it hands the signal on: a fault
    to what had it before (std's overflow check, which still prints
    "has overflowed its stack", then the default action, so the core is
    made as it would have been), a sent one to the default action, so
    the program ends as its sender meant (std alone lets a program go
    on after a sent SIGSEGV). An abort after a panic is that panic's
    crash: not reported twice.
- **stderr to the journal when it goes nowhere.** A program whose
  stderr is /dev/null (or closed), as everything started by Tracker,
  Deskbar or another app is (`olo_xdg::open::spawn_app`), connects it
  to the journal's stream socket itself (`/run/systemd/journal/stdout`,
  the stream protocol's header naming it for the program), non-blocking
  so a journal that can't keep up loses a line rather than holding the
  program up. Before, such a program's log and its panic's message went
  nowhere (Viewer's crash in TODO left no trace). A crash's own line
  goes there marked an error (`<3>`), "PROGRAM: SIGSEGV; the report is
  PATH". The kit's log writes colour codes only to a terminal.
- **Told**: the session's `olo-doctor --watch` follows the folder by
  inotify and tells of each new report once ("Viewer crashed", what and
  in which thread, the program's own icon; more than three at once in
  one note); its click opens Log Viewer on the program's lines from
  just before (`olo-logs --query "program:P since:Nm"`).
  `olo-doctor --crashes [--json]` lists them for a terminal or an
  agent.

Cost: nothing while the program runs (no thread, timer or check);
installing at start measured at 18 µs median (35 µs when it also
connects stderr to the journal; `cargo test -p olo-crash --test crashes
install_cost -- --ignored`), and 33 KB of code in a binary.
`crates/crash/tests/crashes.rs` crashes children for real (a panic, a
worker's panic, a segfault, a sent SIGSEGV, an abort, a stack
overflow; no core, as `PR_SET_DUMPABLE` 0) and checks each report and
how the program ended, and the journal stream against a stand-in
socket.

## Layout

| Path | What |
|---|---|
| `crates/app_server` | The compositor (Smithay, forked from Anvil). Draws the BeOS window decorator (yellow tabs), manages windows/workspaces, serves IPC. |
| `crates/session` | `olo-session`: starts and supervises app_server, Deskbar and the desktop; in-place reload and upgrade (see below). |
| `crates/protocol` | JSON-lines IPC between app_server and our apps (`Client`, `Request`, `Reply`, `Event`, `Settings`). |
| `crates/theme` | `Palette` (light/dark), BeOS tint helpers, decorator metrics, font lists. |
| `crates/ui` | `olo-ui`, the Interface Kit on Olo's GPUI: windows, layer surfaces and popups, menus and the menu bar, BeOS widgets, icons, the agent tree and socket, upgrades. **All apps are built on it** ("The GPUI kit" below). |
| `crates/kit-base` | `olo-kit-base`: what the kit has that doesn't draw, re-exported by olo-ui under the same names: `instance` (one process per app), `settings`, `notify`, `files`, `places`, `watch`, `tracker`, `metrics` (the spacing system), `Proxy`, `search` (the query language, facets and suggestions), `masonry` (cards in columns), and `store` (an app's own files, written whole), `commands`, the socket agents and the command palette read apps through (its protocol, server and client), with `review`'s findings and `layout_review`'s checks, `list` (a list's selection and type-select), `spell` and `keys` (the keyboard profile's keys). No GPUI. |
| `gpui/` | Olo's GPUI, which the kit is built on: GPUI 0.2.2 and blade-graphics 0.7.1, Olo's own from now on (Wayland only, idle windows draw nothing); `gpui/README.md` says what changed. |
| `crates/xdg` | `olo-xdg`: freedesktop plumbing shared by app_server and the kit: data dirs, `.desktop` entries, icon lookup/decoding (`load_icon`, in the current theme: `themes`), our own apps (`apps::our_app`), MIME types (`mime::for_path`), opening files and URLs with the right app (`open::open`, `open::handler_cached`: never `xdg-open`; the person's own choice first (`open::set_default`, kept in `~/.config/olo/default-apps.list` in mimeapps.list's form and copied into `olo-mimeapps.list` for other programs; `handler_cached` notices it changed, in any process), then Olo's apps, except that links and web pages go to the browser chosen in mimeapps.list, else `olo-chrome`; Open With's few apps for a kind, `open::handlers_for`: as the person set them (`set_offered`, Preferences ▸ Default Apps ▸ Open With), else the one it opens in, Olo's own for it, then those whose entries claim it or a broader kind, by name, eight at most; worked out off the UI thread by `open::warm` (a folder's kinds, as Tracker lists it) and read by menus from `offer_cached`; `open::kind_of` names a kind by the file's name alone; `open::every_app` is every app whose command takes files (`DesktopEntry::takes_files`), Olo's first, and `Handler::icon` its icon; the person's own kinds of file are `mime::own_kinds` (`set_own_kinds`; `mime::type_for_extensions` keeps the type its extensions all share, else makes one of Olo's own, `application/x-olo-kind-EXT`, which `mime::from_name` gives first); desktop entries are kept until an applications folder changes, `desktop_entry::all_cached`; and the same for other programs: `open::for_others` gives the hidden `olo.viewer`, `olo.styled-edit` and `olo.tracker` desktop entries, each listing every type shared-mime-info knows that Olo opens with it, and an `olo-mimeapps.list` making them the defaults, which GIO, `gio open`/xdg-open and xdg-desktop-portal's OpenURI read before `mimeapps.list` while `XDG_CURRENT_DESKTOP` names Olo; olo-session writes them into `~/.local/share/applications` at start, makes that folder's `mimeinfo.cache` again when they change (`open::mime_cache`, as update-desktop-database would: GIO only finds an app for a type through it) and removes ones it wrote that are no longer wanted, `X-Olo-Written=true`; and, once Olo's Chrome is installed, `open::offer_browser` writes `~/.local/share/applications/olo-mimeapps.list` naming it for web pages and links, read after every `mimeapps.list` in the config directories so a browser the person chose wins, but ahead of the system's lists), `find_program`, `spawn_detached` (`spawn_detached_app` and `open::spawn_app` for a desktop entry's program, in a scope of its own: `scope`; Olo's own programs keep the launcher's stderr, the session's journal, so what Deskbar, Tracker or the palette starts logs like the rest; others' goes nowhere). |
| `crates/system` | `olo-system`: the system over its own protocols, in process: logind (`power`: reboot, power off, suspend, inhibitor locks), NetworkManager (`network`: status, join including 802.1X, forget), the sound server (`audio`: volume, mute, `watch` change feed), dconf writes, the D-Bus/systemd activation environment. All blocking: call from worker threads. |
| `apps/airpods` | `olo-airpods`: AirPods, natively (see "AirPods"): the session's service and its command line. |
| `apps/voice` | `olo-voice`: the voice service (see "The voice service"): every microphone through voice processing, and its command line. |
| `apps/installer` | `olo-installer`: Olo Setup, a stock Fedora Workstation made Olo in one full-screen program (see "Olo Setup"). |
| `apps/dconf` | `olo-dconf`: Olo's dconf service (`ca.desrt.dconf`), GSettings programs' settings written into dconf's own database in place of GNOME's dconf-service (see "Other toolkits' programs"). |
| `apps/secrets` | `olo-secrets`: Olo's own Secret Service and Secret portal, programs' secrets kept in place of GNOME Keyring unless Preferences ▸ Security says otherwise, its store opened by the login password (see "Programs' secrets"). |
| `crates/pam-keyring`, `crates/login-key` | `pam_olo_keyring.so`: the login password opens Olo's keyring, the key it gives handed to olo-secrets in the login's kernel keyring; `olo-login-key`: that key's derivation, shared (see "Programs' secrets"). |
| `crates/camera` | `olo-camera`: cameras in process: listed from sysfs (nothing opened till one is asked for), a V4L2 stream (MJPEG decoded by zune-jpeg, or YUYV converted) as BGRA frames; `OLO_CAMERA_FAKE=1` puts a stand-in first ("Test Camera", a moving pattern at 30 frames a second) for tests and headless sessions, which never open a real camera. For a recording's camera bubble (Screenshot's `bubble.rs`), later face sign-in. |
| `crates/face` | `olo-face`: face unlock natively: the infrared camera over V4L2, its emitter through its UVC extension unit, YuNet and SFace run in process (tract), and `olo-faced`, the root service on the system bus. See "Face unlock". |
| `crates/pam-face` | `pam_olo_face.so`: face unlock in sudo's and polkit's PAM stacks (asks `olo-faced`). See "Face unlock". |
| `crates/fido` | `olo-fido`: FIDO2 security keys (a YubiKey) natively: CTAPHID over hidraw, the CTAP2 Olo uses, and the account password sealed by a key's `hmac-secret`, so a touch can stand for typing it. See "Security keys". |
| `crates/eld` | `olo-eld`: an AAC-ELD decoder in Rust (AirPods' microphone stream: mono, 480 samples a frame, no SBR). Huffman tables vendored from Symphonia (MPL-2.0), the standard's band, TNS and window tables transcribed (`tables.rs`); checked against libfdk-aac's decode of a libfdk-made stream (`tests/fdk.rs`: 85 dB) and, with `--ignored`, of a real capture. |
| `crates/voice` | `olo-voice`: what Olo does to every microphone, 10 ms at a time: echo cancellation (its own fullband Kalman filter), noise suppression (DeepFilterNet3, vendored in `vendor/deep_filter`), level normalization (WebRTC's AGC2, vendored in `vendor/sonora`). See "Voice processing". |
| `crates/pool` | `olo-token-pool`: token pooling for Claude Code and Codex (see "Token Pool"): the accounts, their sealed store, the drain order, the loopback gateway and the clients' proxy (its own certificate authority, and which program is connecting, from /proc). |
| `crates/ai` | `olo-ai`: how AI features reach their models (see "AI features"): `ask(&settings.ai, &feature, &Ask)` → the answer's JSON. Blocking; call off the UI thread. |
| `crates/preview` | `olo-preview`: native previews of every file kind (text, images, PDF, audio, video, archives, office documents, fonts). Its decoding needs no toolkit (pictures are `Bitmap`s, premultiplied RGBA; token classes are `olo_theme::Syntax`, coloured by `Palette::syntax`)); its views (`ui::PreviewView`, `ui::rich::RichView`, `ui::table::TableView`) are behind its `gpui` feature (the Viewer's and Quick Look's). Source code is coloured by syntect's bundled Sublime grammars plus Olo's own in `src/syntaxes` (TOML, with Cargo.lock and other lock files; INI, with systemd units, .desktop entries and git and editor configs), its scopes mapped onto the theme's token classes (`text::classify`). Spreadsheets and CSV (`table.rs`, sheets shared as `Arc`s): every column, scrolled sideways; the first row as the columns' names when it looks like a header (all text, with numbers or blanks below); number columns lined up on the right; a bar with Search (every word, in all columns or the one chosen beside it; Escape clears, then lets go) and how many rows show ("14 of 120 rows"); a header click sorts (again to reverse): numbers as numbers (a thousands comma, a currency sign or a percent allowed) before text, text naturally and without case ("file2" before "file10"), empty cells last, ties in the file's order. A sheet of more than 40,000 cells is searched and sorted on a worker (cancelled when asked again), the rows shown until then staying put and the bar saying "Searching…". Quick Look's own keys (Space, the arrows, F) stand aside while a field has the keyboard. Formatted documents (Markdown, and Word, OpenDocument, RTF and PowerPoint text) are blocks (`doc::Block`) set as a page by `rich.rs`: Word, OpenDocument text and RTF are read by `olo-richtext` (`doc::rich`: headings, lists numbered as they count, quotations, neighbouring code lines as one block, tables as tables; empty paragraphs are spacing, left out), slides by `doc.rs`'s own zip and XML; Markdown is CommonMark with GitHub's tables (their alignment, and styled cells), task lists, strikethrough and alerts (`> [!NOTE]` and the rest, a bar and title in the alert's hue), typographer's quotes and dashes; raw HTML keeps its text (`<br>` breaks, `<img>` is an image, comments go); code blocks are coloured by their fence's language (`text::highlight`, the text preview's grammars); pictures beside the document are shown (local files only, never fetched; a remote one's alt text shows instead), resampled exactly to their device size. The page is at most 680 points wide, centred, body text 13 points on a 20-point line, headings 26 to 13 points with room above them (rules under the first two), lists with hanging indents (bullets drawn, numbers lined up on the right, tasks as `check_box_look`s), inline code on a tint with a small no-break space either side (a line doesn't break between "(" and code), and links in the link hue, underlined under the pointer; they open with `olo_xdg::open` (a relative one beside the document) and a `#heading` scrolls to it (GitHub's anchors). Blocks are laid out once for a width, scale, look and font generation: those in view first (at most 5 ms), the rest 3 ms a frame after, and a spinner shows over a part not laid out yet; only blocks in view are drawn. Agents read each block (`Heading`, `Paragraph`, `ListItem`, `Code`, `Table`, `Image`) and click its links (`Link`). Text and formatted documents' text is selected as anywhere in Olo (a drag; a word with a double-click, a line with a triple; ⇧ extends; an I-beam over it), drawn behind the text in the kit's selection (`style::selection_behind`, the text fields' too): the text preview's places are a line and a byte, hit by each line shaped as drawn (any characters, not only monospaced ASCII), a whole line's selection taking its line break; a document's are a block, one of its texts, a line and a byte (`WrappedLine::closest_index_for_position`, painted row by row from `position_for_index`, just behind each text so a code block's ground stays under it). `PreviewView::copy` gives what's selected (a document's partial selection: paragraphs apart, a list's items a line apart, a table's cells by tabs and its rows by lines; Select All: every block's own text, laid out yet or not), and `open_find` the kit's find bar (`widgets::find_bar`, Viewer's PDFs' too): a text's matches by line, a document's over what's laid out (found again as more is, "…" until it all is), each marked, the one shown selected and centred. With nothing selected or found the views do nothing more a frame (TODO.md in the rich view scrolling at 60 frames a second: 484 to 535 µs a frame before, 499 to 522 after; the text view's listeners cost about 25 µs a frame). Used by Tracker's Quick Look (`PreviewView::new(item, generation, false, cx)`, images capped at 3200 px) and the Viewer (zoomable: full resolution up to the GPU texture limit, zoom and pan through its `ImageView`). Video (`video.rs`, H.264 in mp4 or Matroska, AV1 and VP9 in Matroska/WebM or mp4, HEVC in mp4 (`hvc1` and `hev1`: `media::mp4_hevc` walks the boxes to its `hvcC` and `colr`, which the mp4 crate skips) or Matroska (`V_MPEGH/ISO/HEVC`, the record in its CodecPrivate, its `Colour`), with `media.playable` set; the mp4 crate doesn't know AV1's `av01` sample entry, so `media::mp4_av1` walks the boxes to it for the track and its `av1C` sequence header, sent before each key frame): a worker demuxes and decodes (OpenH264, rav1d through `olo_images::av1::Decoder`, filmcraft-vp9 through `olo_images::vp9::Decoder`, or rust_h265 through `olo_images::hevc::Decoder`, whose pictures are `av1::Frame`s too: HEVC's from its decoder configuration record's parameter sets, on four threads in display order, its matrix and range the container's, else BT.709 from 720 lines and BT.601 under) up to six frames ahead, giving each picture the earliest outstanding presentation time (VP9's carry their sample's own, and times left behind by samples that showed nothing are dropped) (OpenH264 returns display order; it must be made with `Flush::NoFlush`, as the crate's default flush after every packet breaks B-frame reordering and every B-frame then fails), mp4 times shifted by the track's edit list; `Video::tick` shows frames against a clock that follows the sound's reported position when an `audio::Player` is playing it and runs alone otherwise (no sound server in tests, silent files). Seeks go to the key frame before the target (Matroska: scans back for one, further each time) and drop pictures before it, all but the one still showing at the target (a still screen in a screen recording is one picture, seconds long: seeking into it showed the next change; `seeks_into_a_long_picture`): the last before it is held (made) unless its sample's duration (an MP4's, a Matroska block's own) says it ended first, and queued at the target's time when the next starts after it; past the last picture's end, that one is decoded again (`Lead`). Holding every picture cost a 4K H.264 seek a sixth more (`seek_cost`: 386 ms against 329; with durations, 317); a paused seek still shows its frame. `OLO_SAMPLES=dir cargo test --release -p olo-preview video -- --ignored` checks seeking on test.mp4/mkv (one key frame) and gop.mp4/mkv (one a second), made with ffmpeg's testsrc2, plus av1.webm (svt-av1, Opus sound), vp9.webm (libvpx with `-auto-alt-ref 1 -lag-in-frames 25`, so hidden frames in superframes), av1.mp4 (`av1_in_mp4`: 4 s of testsrc2 through libsvtav1, a key frame a second), vp9.mp4 and vp9-10bit.webm (`vp9_plays`: profile 0 in mp4, profile 2 in WebM; every picture, posters, facts), hevc.mp4, hvc1.mp4, hevc.mkv and hevc-10bit.mp4 (`hevc_plays`: 1080p HEVC with B-frames as `hev1`, `hvc1` and Matroska, remuxed by `ffmpeg -c copy`, and JCT-VC's Main 10 WPP_D in an mp4; every picture, seeks, posters, facts) and bframes.mp4 (any x264-made H.264 High clip with B-frames; `b_frames` checks every picture comes out, in order); `keeps_up` needs av1-1080.mkv (1080p AV1), vp9-1080.webm (1080p VP9), gop.mp4 and av1.mp4 to decode at well over 30 frames a second (72 on the M2 Max, 195 now; VP9 120; av1.mp4 600; `OLO_ONLY=a,b` picks samples; the YUV to RGB fast path is fixed-point with tables, checked against the float path by `yuv_fast_path`). Drawn by the GPU from its planes (Ben, 2026-11-06: "the viewer app can't even play 4k video at 60fps smoothly"): a window's player asks for `Options::yuv`, and each SDR 4:2:0 picture comes as `Picture::Yuv`, a `gpui::YuvFrame` made on the worker (luma packed, Cb and Cr interleaved; 10-bit samples in the high bits of 16; H.264's limited-range BT.601 as OpenH264's, the others' matrix and range as `olo_images::av1::Frame::yuv420` says), which the window paints with `Window::paint_yuv`: GPUI's Linux renderer keeps two pairs of textures a stream (R8/RG8, or R16/RG16 unorm, added to blade), uploads each new picture into the pair the frame before isn't reading, and the surface shader makes RGB with the picture's own matrix (`YuvFrame::rgb_matrix`). A 4K picture is 12 MB rather than 33 of RGBA, and nothing converts it on the CPU; HDR, wide-gamut and 4:2:2/4:4:4 pictures keep the RGBA path. Measured on 4K60 clips (`make_4k60`: 10 s of 3840 × 2160 through NVENC) on a headless 4K screen: H.264 29.6 frames a second before, 60.0 after; HEVC 24.4 to 59.8; AV1 52.2 to 60.0; the window's own work 18 to 25 ms a frame before (making the picture BGRA and a texture of it), 0.5 ms after. Decoded on NVIDIA's GPU where there is one (`olo_video::nvdec`: NVDEC's API through vendor/nvdec-sys's declarations, `libnvcuvid.so.1` and `libcuda.so.1` opened at run time, nothing linked; its parser takes the stream, HEVC as Annex B with its parameter sets before each key frame; each picture NV12 or P016, copied from the GPU into a `YuvFrame`; its colour from the stream's own H.273 numbers): H.264, HEVC (8- and 10-bit), VP9 and AV1, each asked of the GPU first (`cuvidGetDecoderCaps`). What decodes is chosen by `video::Device` (`Options::device`): `Default` follows Preferences ▸ Power ▸ Graphics ▸ Video Playback (`settings.video.decoder`: `auto`, `nvidia`, `intel`, `cpu`), `Automatic` is the built-in GPU where it decodes the video, else NVIDIA's (on battery too, unless `settings.video.nvidia_on_battery` is off: Ben, 2026-11-06), else the processor, or one named; a decoder that fails mid-stream hands over to the processor at the last key frame, the video playing on. `Video::decoding` says what is decoding ("NVIDIA's GPU (NVDEC)", "the processor"), `PreviewEvent::Decoding` when it changes, and `PreviewView::set_decode_with` starts the video again on another at the same place, playing or not (the Viewer's View ▸ Decode With). Measured on the 4K60 clips: all at 60 frames a second either way; the process's CPU HEVC 246% of a core on the processor to 41.6% on NVDEC, H.264 100% to 29%, AV1 368% to 26%. Every codec decodes on Intel's (or AMD's) GPU too (`olo_vaapi::decode`: VA-API through vendor/va-sys, libva opened at run time). Fedora's iHD (and Mesa) leave out H.264 and HEVC for their patents, and its Mesa compiles them out of Vulkan Video too (ANV shows AV1 and VP9 decoding only with `ANV_DEBUG=video-decode`); RPM Fusion's `intel-media-driver` (nonfree) has them all, in `/usr/lib64/dri-nonfree`, which libva reads before Fedora's folder, and Olo Doctor installs it (`intel_video`: an Intel GPU on Fedora without it; the fix adds RPM Fusion's free and nonfree repositories by their release packages, as rpmfusion.org says, `Step::InstallRepository`, then the driver through PackageKit). H.264 and HEVC go through cros-codecs' stateless decoders (vendor/cros-codecs: its parsers and what a decoder keeps between pictures, the decoded picture buffer, picture order, reference lists and marking, bumping; its own backends and encoders, which link libva and libgbm, gated off) driving a backend of ours (`decode/stateless.rs`: a picture in one of the session's surfaces, back in the free list when nothing holds its handle; `h264.rs` and `h265.rs` fill VA-API's picture, scaling list and slice parameters as ChromeOS's backend does, HEVC's slices a slice behind so the last says so). AV1 and VP9: cros-codecs' parsers read the OBUs and frames, and each picture's parameters are filled as FFmpeg's and ChromeOS's decoders fill them; references are eight slots of surfaces; AV1's film grain is applied by the driver into a display surface of its own; VP9's superframes and shown-again frames are decoded as libvpx does. A picture comes back by the driver's copy engine (`vaCopy`) into a linear surface in memory of ours (`USER_PTR`, the chroma right after the luma rows, as iHD lays it whatever offset it's given), then into planes: mapping the decoder's own (tiled) surface had the processor untile it, 7.5 ms a 4K picture, and `vaGetImage` 7 ms, so 4K60 dropped to 58; the video processor isn't a copy (it changed pictures). A GPU decoder's pictures are held, not copied out (`Decoder::hold`, `Nvdec::hold`; `OLO_VIDEO_HELD=0` copies, to compare): each decoder keeps up to 16 pieces of page-aligned memory (VA-API's linear surfaces, NVDEC's buffers), copies a picture into one nothing holds, and hands it on as it is (`Picture::held`: its planes' offsets and row pitch), as a `gpui::YuvMemory` on the `YuvFrame`; GPUI's renderer imports that memory as a Vulkan buffer (blade's `import_host_buffer`, VK_EXT_external_memory_host) and copies it into the stream's textures on the GPU, letting both go once the GPU is past that frame (a picture it can't import is staged as before, at its pitch). NVDEC's memory is registered with its CUDA context (`cuMemHostRegister`: its copy goes by DMA, not through the processor as into pageable memory), the context shared with that memory and destroyed when the last of it goes (`Cuda`), and made with blocking sync (waiting for a copy, the thread sleeps rather than spins). 4K60 AV1, the Viewer's share of a core, copied → held: Intel's GPU 21.9% → 7.1%, NVIDIA's 30.3% → 21.9%, pinned 13.8%, blocking 7.0% (its video thread 19.9% → 1.1%; 4K60 VP9 7.0%; nothing dropped). `crates/video/tests/reference.rs` decodes reference streams on every device here, each picture against the reference decoder's MD5 (libvpx's VP9 profile 2 and 1080p in tiles, libaom's AV1 10-bit, the HEVC conformance suite's Main 10 in wavefronts; NVDEC takes VP9 only from 128 × 128, said before trying); cros-codecs' per-picture HEVC reference-set warnings are debug notes (ordinary streams do it). `crates/vaapi/tests/decode.rs` checks every picture's MD5 against the reference decoder's (cros-codecs' AV1, VP9, H.264 and HEVC streams: B-frames, H.264 High, HEVC films, VP9's shown-again frames, a reset; a codec no GPU here decodes is said and passed: `LIBVA_DRIVERS_PATH=DIR` points the test alone at RPM Fusion's driver unpacked there). `Automatic` takes the built-in GPU where it decodes the video, else NVIDIA's (on battery too); `Video::decoders` says which GPUs could (the Viewer's menu enables those). 4K60 on the ThinkPad's Intel GPU (Panther Lake, RPM Fusion's iHD 26.3.5), the process's share of a core: H.264 25% (NVDEC 29%, the processor 100%), HEVC 21% (41.6%, 246%), AV1 22% (26%, 368%); a minute of each, with six ahead and the clock waiting for the first picture, played every picture but one (H.264 3600 of 3600, HEVC 3599, AV1 3600, none late; NVDEC's H.264 5 dropped, all at the switch to it), where at two ahead NVDEC had dropped 155, 68 and 176. On the processor, rav1d's and OpenH264's x86 assembly (SSE4 and AVX2, chosen at run time) is built in: nasm assembles it (tools/setup.sh installs it; OpenH264 leaves its assembly out without saying so when there's no nasm, and rav1d's `asm` feature fails the build). `keeps_up` on the 4K60 clips (`OLO_SAMPLES=/var/tmp/olo-4k OLO_ONLY=…`): AV1 32 → 85 pictures a second, H.264 45 → 53–98 (OpenH264 decodes on one thread, as fast as the core the scheduler gives it; its threads crash it), HEVC 108 → 140 (rust_h265, no assembly; the machine was busier for the first); the Viewer playing 4K60 AV1 on the processor 368% → 175% of a core. A frame keeps 8-bit video's samples as bytes (`av1::Planes`, words only deeper; every conversion runs over either through one macro): it had widened them to 16 bits and the queue counted each 4K picture 50 MB, one ahead, so 4K60 AV1 on the processor dropped 1,510 of 3,600 pictures in a minute, now 3 (its video thread 57% of a core → 17.5%). The worker keeps up to six pictures ahead within 80 MB (`QUEUE_BYTES`: six 4K pictures as planes; at 24 MB a 4K picture had two, 33 ms at 60 frames a second, and every hiccup of a decoder's dropped pictures, NVDEC's H.264 150 a minute), and with no sound to follow the clock waits for a late decoder up to 200 ms (`HOLD`: starting, a stall), so a picture shows late rather than the ones after it being passed over; one that can't keep up at all is caught up with by passing pictures over. How smoothly a video played is counted (`Video::playback`: pictures shown, dropped, already late when their turn came, and shown late, more than 25 ms after their time) and logged when the player goes ("video: 3600 pictures shown, 0 dropped, 0 late; decoded on Intel's GPU (VA-API)"); `make_4k60` makes longer clips with `OLO_SECONDS`. Audio decodes with symphonia plus `opus.rs` (opus-decoder, OpusHead parsing and pre-skip trim) for Ogg Opus. `audio::Player` speaks the PulseAudio protocol itself, and its client says who is playing as libpulse does (`application.process.id`, `.binary`, `.user`, `.host`, beside the app's name and icon): Fedora's uresourced follows the programs playing sound by their process, and crashed at every Olo login on the start-up sound's stream (a null process id). Picture editing (`edit.rs`): an `Editor` keeps a list of `Op`s (rotate, flip, crop, resize) with undo/redo and a saved mark, and a worker that loads the full picture once, applies new steps to its last result, sends `EditEvent::Shown` (downscaled for the screen) for `PreviewView::show_edited`, and writes PNG, JPEG (quality 92), WebP, BMP, TIFF or GIF atomically (`write`, through a hidden `.name.saving` file); `edit::editable` says which files it can write back. The Viewer's crop overlay (`crop.rs`: drag a frame, move it or its edges) sits where `ImageView::picture_bounds` says the picture is, `set_still` stopping pan and zoom while it is up. The Viewer's Image menu and File ▸ Save / Save As / Revert use it; Quick Look's footer has Rotate Left / Right (⌘L / ⌘R), which save at once. |
| `crates/richtext` | `olo-richtext`: formatted text documents in and out, natively (zip and quick-xml): Word (`.docx`: styles, numbering, hyperlinks' relationships), OpenDocument text (`.odt`: automatic and named styles, nested lists, `text:s` for runs of spaces) and RTF (`rtf.rs`: a reader of groups and control words — font and colour tables, the style sheet's names for kinds or else `\outlinelevel`, lists from the list table or Word 95's `\pn`, links from `HYPERLINK` fields, tables from `\intbl` paragraphs, `\cell` and `\row` (`\trhdr` a header), any Windows code page by `\ansicpg` or a font's `\fcharset`, `\u` with its fallbacks skipped, surrogate pairs; pictures, headers, footers, notes, hidden text and unknown `\*` destinations left out — and a writer of plain ASCII with a style sheet, a list table per numbered list and every paragraph's look spelled out, as RTF readers take a paragraph's look from its own words, not its style; the reader takes each kind's look off again). A `Document` of `Paragraph`s (kind: body, title, heading 1–3, quote, code; a list, bulleted or numbered, and its level; an indent; alignment; or a `Table` in the flow: rows of cells of paragraphs, the first row maybe a header, and the columns' widths if they have their own (`widths`, shares of the table after `tidy`: Word's `w:gridCol`s, OpenDocument's column styles, RTF's first row's `\cellx` edges), written ruled across the page in each format) of `Span`s (bold, italic, underline, struck out, size, colour, font, monospace, link), indents beyond the levels in points (`left`, `first`, `right`; read as whole levels and the rest) and tab stops (`Tab`: left, centre, right, decimal), and the document's font. `read`/`write` go by the file's extension; `Document::tidy` runs same-styled spans together and makes a line break a new paragraph (in cells too), and gives every row a cell for each column and every cell a paragraph. Light on purpose (StyledEdit links it; `preview` is far heavier), and round-trip tested; its files open in LibreOffice as they look in StyledEdit (`OLO_RICHTEXT_OUT=dir cargo test -p olo-richtext writes_samples` writes a sample of each; `OLO_RICHTEXT_READ=file cargo test -p olo-richtext reads_named_file -- --nocapture` prints how another program's file reads). |
| `crates/syntax` | `olo-syntax`: source code coloured by syntax, in process: tree-sitter's grammars for 43 languages (Rust, C, C++, Go, Python, JavaScript, TypeScript and TSX, shell and fish, Ruby, Java, Kotlin, Swift, C#, PHP, Lua, SQL, HTML, CSS, JSON, YAML, TOML, XML, Makefiles, Dockerfiles, CMake, Scala, Haskell, Zig, Elixir, Erlang, Nix, R, Julia, OCaml, Dart, Objective-C, Protocol Buffers, PowerShell, INI, diffs), each compiled in, with its highlight query (its crate's, or `queries/`: Kotlin's is Olo's own) whose captures map to the palette's token classes (`class_of`: `function.method.call` a function, `variable` plain inside a string). `LANGUAGES`; `detect(path, first_line)` (a whole name like `Makefile`, the extension, a `#!` line's interpreter, `env -S` and versions too); `by_name` (an id, alias or extension: what a Markdown fence says). A `Highlighter` keeps one text's tree: `edit(&Edit)` per change (`Edit::new` with the lines it's on, or `Edit::in_text` counting them), `job(text)` the parse to run on a worker (one out at a time; a language's query compiles in its first, up to half a second for Swift's), `done(parsed)` takes the tree (edits made meanwhile applied to it; what changed: `Changed`), `classes(text, range)` the query over just those bytes (inner captures and a node's first pattern win). 100,000 lines of Rust: parsed in a second, a keystroke's reparse 16 ms (both on the worker), a screenful coloured in 0.35 ms. The kit's `code` module puts it in the editor. |
| `crates/print` | `olo-print`: printing, natively. `Page`s of `Item`s (text at a baseline with its font, size, colour, width and lines; rectangles; lines; pictures, a PNG, JPEG, GIF or WebP file's bytes in a box), in points from the top left, made into a PDF by `pdf` (krilla: fonts found by fontdb, Olo's own for text naming none, embedded and subset, text stays text; a character the font lacks drawn from a face that has it, `ttf-parser` telling; bold from a variable font's default instance stroked as well as filled, as browsers print it). `ipp`: CUPS spoken to over IPP on `/run/cups/cups.sock` (else `localhost:631`; `OLO_CUPS_SOCKET` for a test's stand-in, as `talks_to_cups` has): `printers()` (name, description, state, paper), `default_printer()`, `print(printer, pdf, &Job)` (copies, paper, pages, sides). `panel::PrintPanel`: the Print window (printer or PDF File, paper, copies, pages; the printers asked for on a worker); `Paper`, `PAPERS` and `local_paper()` (US Letter where it is used, else A4). |
| `apps/<name>` | One crate per application, binary named `olo-<name>`. Picked up by the workspace glob. |
| `vendor/` | Patched copies of dependencies (`[patch.crates-io]`), each explained in `vendor/README.md`; drop them when upstream has the fix. |
| `data/icons` | Icon set, pre-scaled to device sizes (`data/icons/<px>/<name>.png`). |
| `data/themes` | The other looks of the icons and wordmark (`data/themes/<id>/icons/<px>/<name>.png`, `brand/`), chosen in Preferences ▸ Themes; see "Themes" in [ICONS.md](ICONS.md). |
| `data/session` | `olo.desktop` for the login screen (installed by `tools/install-session.sh`). |
| `tools/` | `install-session.sh` (release build → `~/.local`, session entry → `/usr/share/wayland-sessions`, an atomic system's `/usr/local/share/wayland-sessions`), `dev/tsession` (headless UI testing), `genicon.py`/`genicons.sh`/`mkicons.py`/`iconsheet.py` (icons, see [ICONS.md](ICONS.md)). |

Build: `cargo build -p olo-<app>` (plus `cargo build --bin olo-app-server` once, for headless testing; as root set `CARGO_HOME=/home/ben/.cargo` to share the registry) (repo is owned by `ben`; when running as
root use `sudo -u ben bash -lc 'cd … && cargo build …'` in the main checkout).
How long a release build takes after a change (ThinkPad, 16 threads,
2026-11-05, the whole workspace): the kit (olo-ui) 92 s, app_server 47
s, a leaf app 2.8 s. A kit change is bound by CPU, not by any one step:
olo-ui 21 s, then some 35 apps compiled and linked at once, about 1,000
CPU-seconds in all (olo-portal alone, with the machine to itself, is 15
s, half of it local ThinLTO). Tried and not kept: `incremental = true`
(103 s, 46 s, 3.4 s) and one codegen unit a crate (159 s, 190 s, 6.1 s);
the linker is already rust-lld (Rust's default on x86_64 Linux since
1.90; 0.4 s for olo-portal), kache already caches. So while working on
one app, build just it (`cargo build --release -p olo-<app>`: a kit
change then costs olo-ui and that app, about 35 s), and the workspace
once at the end, for the session's upgrade.
Builds go through [kache](https://github.com/kunobi-ninja/kache), a
compiler cache (`cargo install kache --locked`, then
`kache init --no-shell --no-service`, which sets `build.rustc-wrapper` in
`~/.cargo/config.toml`): a crate compiled once with the same inputs is
restored instead of rebuilt, in any target directory or worktree. It is
ben's setup, so build as ben (as above); `kache doctor` checks it,
`kache stats --last-build` shows the hits, `KACHE_DISABLED=1` bypasses it.
Know what it is for: a cache pays off when the same inputs come back
(a branch switched and switched back, a revert, a second checkout or
target directory), not in a straight edit-build loop, where every build's
inputs are new; over a day of such work it served 0.6% of crates for ~2%
overhead (measured 2026-11-01: ~2 min of key hashing and ~12 min of
storing in 12.6 h of compiling). Its daemon is a login service (`kache
daemon install`, done by `tools/setup.sh`), so the cache stays under its
cap without anyone running it by hand. Blaze (indexable-inc/blaze, a
build daemon with its own compiler) was tried the same day and dropped:
it needs more memory than this laptop can spare (OOM at a 10 GB cap
loading the workspace) and could not compile GPUI's styles
(`<f64 as Into<f32>>::into` in `ToTaffy`); cargo's loop is 1–3 s for an
edit to an app, the kit or GPUI in a debug build, and ~90 s for a
release build of every app after a GPUI edit, which is linking 26
binaries of 200 MB.
Builds are kept lean on memory and disk. Dev builds keep only line tables (file and
line in panics and backtraces), and dependencies keep no debuginfo, so debug binaries
stay small and cheap to link. Full debuginfo made each one 600–850 MB, and twelve
links at once ran a 30 GiB laptop out of memory. On ben's machine,
`~/.cargo/config.toml` also caps `build.jobs` at 6 and links with lld
(`-C link-arg=-fuse-ld=lld`). `~/.config/kache/config.toml` caps the cache at
15 GiB. The target directory is swept by `tools/dev/sweep`: cargo never
deletes an artifact, so every edit to a flag, feature or dependency leaves
the old crate's rlib, binaries and build script behind under a new hash
(olo's `target/` reached 435 GB; one build's live set is ~110 GB, debug
and release together). The sweep removes every unit no build has
considered for two days (cargo touches `.fingerprint/<unit>/invoked.timestamp`
on each build that wants a unit, compiled or fresh), with everything in
`deps/`, `build/`, `examples/` and `incremental/` carrying its hash; the
first run freed 172 GB in 11 s. `tools/dev/sweep --install` (also done by
`tools/setup.sh`) puts it on a daily systemd user timer, `olo-sweep.timer`;
`--dry-run` reports, `--days N` changes the window, and a directory is only
swept when it has cargo's `CACHEDIR.TAG`. Scratch target directories
elsewhere (`/var/tmp/...`) are the person who made them's to remove.

## Writing an app

Apps are written on the GPUI kit (crates/ui, `olo-ui`, on Olo's GPUI:
"The GPUI kit" below has its widgets and what each does):

```rust
use olo_ui::{Menu, MenuBar, WindowOptions, instance, widgets::Button,
    gpui::{AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, Window, div}};

struct MyApp { /* state */ }

impl Render for MyApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        div().size_full().flex().flex_col()
            .child(MenuBar::new(vec![Menu::new("File").item("Quit", "quit").key("q")], move |cmd, _, cx| {
                if cmd == "quit" { cx.quit() }
            }))
            .child(Button::new("OK").default(true).on_click(move |_, cx| me.update(cx, |_, _| { /* … */ })))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward("olo.myapp", &args) { return }     // app id: olo.<name>
    olo_ui::run("olo.myapp", |cx| {
        olo_ui::open_window(cx, WindowOptions::new("My App", 480.0, 320.0).min_size(320.0, 200.0), |_, cx| cx.new(|_| MyApp { /* … */ }));
        instance::listen("olo.myapp", olo_ui::proxy(cx));
    });
}
```

* **`olo_ui::run(id, build)`** starts the app on GPUI (its look,
  keyboard profile and fonts, the agent socket); `open_window(cx,
  WindowOptions, |window, cx| view)` opens a window around a view
  (`WindowOptions`: `min_size`, `max_size`, `floating`, `dialog`,
  `alert`, `child`, `layer`, `under_pointer`…); `proxy(cx)` gives a
  thread a `Proxy` whose messages `message_handler(cx, f)` takes on the
  UI thread; `stay_resident(cx)` keeps it running with no windows;
  `on_upgrade(cx, f)` keeps its state across an in-place upgrade
  (`restored_state` gives it back; see "In-place reload"), which is also
  how the app moves to another display (an agent screen's, or back:
  "Moving windows to and from screens"); `on_move_window(cx, f)` says how
  one window of several goes (a `WindowMove`: the arguments and state a
  copy of the app there opens it with, and `gone`, letting it go here,
  or why it can't), and `refuse_moves_while(cx, f)` keeps the app where
  it is while `f` says why. A view
  draws itself in `Render`; what changes it calls `cx.notify()` (its
  window draws again); work that can take longer than a frame runs on a
  thread (see "The responsiveness rule"). There is no clipboard manager,
  so an app that quits (or closes its last window) while it holds the
  clipboard stays running, windowless, until something else is copied.
* **`olo_ui::clipboard`** (`olo_kit_base::clipboard`): the clipboard for
  a program with no window and no kit loop (a shortcut's screenshot, an
  app that has closed its window). `hold(clip)` puts it on the
  clipboard and serves it until something else is copied, then returns
  (so the program stays running until then); `text(s)` is a `Clip` in the
  forms text pastes as. It goes through
  `ext_data_control_v1`: `wl_data_device` only lets the client with the
  keyboard set the clipboard, and a windowless one never has it. For
  files, as file managers copy them (and Chrome, Electron, GTK and Qt
  paste them): `files(paths, cut)` is a `Clip` of
  `x-special/gnome-copied-files` ("copy" or "cut", then the URIs), KDE's
  cut mark, `text/uri-list` and the paths as text (GPUI's clipboard
  offers each form under its own MIME type);
  `copied_files(mime, data)` reads one of `FILES` back (paths, and
  whether they were cut). `offers(types)` says whether the clipboard has
  any of them, as the app last heard (its windows hear while they have
  the keyboard): for enabling Paste. Tracker copies files so, and its
  Paste takes files (moving cut ones, then holding them copied where they
  went), a picture (`Pasted Picture.png`, of its kind) or text (a
  clipping). `hold_then(clip, ready)` calls `ready` once the display has
  it; `read(types)` is what's copied, in the first of `types` offered.
  For another display (a socket's name in `XDG_RUNTIME_DIR` or its path;
  None, the environment's): `read_all(display)` is what's copied, every
  form of it; `watch(display, now, copied)` hands each thing copied
  there to `copied`, every form, on a thread of its own (the display
  says when: nothing is asked meanwhile), until the `Watch` is dropped;
  `hold_on(display, clip)` serves `clip` there from a thread until
  something else is copied, taking it off again when the `Held` is
  dropped unless `Held::keep`. Agent Screens joins a screen's clipboard
  to the person's with them.
* **`pbcopy` and `pbpaste`** (`apps/pb`), as macOS's, for people and
  agents in any shell: `pbcopy` puts what it reads on the clipboard as
  text and returns once it's there (a copy of itself, in a session of
  its own, serves it until something else is copied: `hold_then`);
  `pbpaste` prints the clipboard's text (`read`). They're
  `olo-pbcopy` and `olo-pbpaste`, installed with Olo's programs; the
  session (`graphical::command_names`) and the installer link `pbcopy`
  and `pbpaste` to them beside them, never over the person's own.
* **Single instance** (`olo_ui::instance`): call `instance::forward(app_id,
  &args)` first in `main` (returns true if a running copy took the
  arguments; exit then), and `instance::listen(app_id, olo_ui::proxy(cx))`
  in `run`'s closure; forwarded launches arrive in `message_handler` as an
  `instance::Launch { args, cwd, .. }`. A launch is answered once the
  running copy's UI thread takes it (olo-ui does that as it hands it to
  the handler; 0.2 ms); a copy ending (its last window closed), or one
  with no handler, drops it unanswered and `forward` returns false, so
  the new process opens it itself rather than it being lost. The
  socket is named for the display (`olo_xdg::display_name()`:
  `wayland-1` also when `WAYLAND_DISPLAY` names the socket by its full
  path; use it for any file kept per display).
  `instance::running(app_id)` says whether a copy is listening. An app
  with several windows should open a new one for `--new-window` (always,
  rather than bringing one forward) and be listed in
  `apps::NEW_WINDOW`: Tracker then offers **New Window** on its program
  (or a link to it) while it runs, and Deskbar in its menu, forwarding `--new-window` to the
  running copy (`apps::app_id_of` maps `olo-x` to `olo.x`). Agents
  do the same with `olo-x --new-window`. Each workspace (of each
  display) has its own
  windows of an app (and so its own group of them): a launch, or opening
  a folder or document the app has a window for, brings forward only a
  window on this workspace, the one the active display shows
  (`workspaces::here(window)`, after `workspaces::follow(cx)`; one on
  another display is elsewhere),
  and otherwise opens a new one here, rather than activate one
  elsewhere, which would go to its workspace or display. (Tracker's folders and StyledEdit's
  bare launch do so, and Terminal's `--tab` goes to a window here; a
  document already open elsewhere is still brought forward there, as two
  windows on one file would disagree.) The kit knows where its windows
  are from app_server's events: `WindowInfo::surface` names each window's
  `wl_surface` as its program knows it.
* **Open in Tracker** (`olo_ui::tracker`): wherever something has a
  folder or file behind it, offer `tracker::LABEL` ("Open in Tracker",
  command `tracker::COMMAND`) in its context menu, and its File menu where
  it has one, and call `tracker::open(path)`: a folder shows in its
  Tracker window, a file selected in its folder's, on the workspace
  showing (`olo-tracker --reveal PATH`, forwarded to the running Tracker
  or starting one). Terminal (a tab's working folder: the tab's context
  menu, the terminal's, Terminal ▸), StyledEdit (File ▸, the saved
  document), the Viewer, DiskUsage, DriveSetup (a mounted volume), the
  backup panel in Preferences and the palette (⌘↩ on a file) use it.
  Other programs reach it through `org.freedesktop.FileManager1`, which
  the desktop's Tracker serves on the session bus (taking the name from
  any file manager that had it; Tracker's `filemanager.rs`): ShowItems,
  ShowFolders and ShowItemProperties are each a `--reveal`, so Chrome's
  Show in folder, Electron's `shell.showItemInFolder` and GTK's and Qt's
  open their file in Tracker. While no Tracker has the name (as Olo
  restarts after an upgrade), D-Bus starts the system's activation file's
  program for it (GNOME's Files on Fedora, which then keeps the name till
  Tracker waits it out): Olo Doctor offers "Always Use Tracker"
  (`checks::file_manager`), the user's own activation file
  (`~/.local/share/dbus-1/services/org.freedesktop.FileManager1.service`,
  which wins over the system's) starting `olo-tracker --file-manager`
  (Tracker serving the name without the desktop or a window of its own,
  staying to answer; passed to a Tracker running already it changes
  nothing), and the session bus told to read it at once
  (`Step::SessionBusReload`). It's the user's, so it applies in another
  desktop's session on this account too (Ben chose it so, 2026-11-05).
  An Olo session names Tracker for it by itself, with nothing to press:
  the same activation file in the runtime folder's services
  (`$XDG_RUNTIME_DIR/dbus-1/services`, read before the user's and the
  system's, gone with the login; olo-session's
  `graphical::name_file_manager`, written once the session's
  environment is exported so a Tracker D-Bus starts finds its display),
  as dconf's is (below); the Doctor counts it as fine. A Tracker D-Bus
  started first is handed the session's `--desktop` and shows the
  desktop (`Tracker::open_desktop`); the session's own `olo-tracker
  --desktop`, which the session looks after, then waits for that one to
  end (`instance::running_pid`, `instance::wait_for_exit`: asleep on a
  pidfd) and ends with it, so the session starts the desktop's Tracker
  again (gone at once, it was started again and again, then left
  stopped).
  A window's yellow tab offers it too, for a window that says what it
  shows (`agent::set_path(window, cx, path)`: StyledEdit's document, the Viewer's file,
  Terminal's working folder; Tracker's own windows don't need it).
  Tracker's Find results have it as Open Parent (⌘↑, also in their
  context menu, with Open and Quick Look). Don't write another; the
  label and the command are the same everywhere, so agents find it by
  either.
* **Following a setting** (`olo_ui::settings`): `current()` reads
  app_server's `Settings` once (a quick local round trip; fine at
  startup), `watch(f)` calls `f` on a thread now, after every
  `SettingsChanged` and after app_server restarts; send what you need
  through `olo_ui::proxy(cx)`. Tracker follows `tracker.default_view` (the view
  new folder windows open in, column view by default) this way.
  `update(edit)` changes them from an app (read, `edit`, `SetSettings`, on
  a thread), and `watch_power(f)` follows `PowerStatus` the same way
  (the screen locking, the displays sleeping).
* **Menus for the command palette** (`olo_ui::commands`, automatic):
  every kit app answers on `$XDG_RUNTIME_DIR/olo-menus-<pid>.sock`
  with the menu bar of its last active window, flattened
  (`commands::menus(pid)`: path, label, command, keys as shown, enabled,
  checked), and chooses an enabled one as if from the menu
  (`commands::invoke(pid, command)`), logging the command and who asked
  (`asked to choose a menu command`, the sender's name and pid). Both
  block for at most a second; call them off the UI thread. (`MenuBar`
  keeps them up to date as it draws: `agent::set_menus`.)
* **What a window shows, for agents** (`commands::tree(pid)`, automatic):
  `{"tree":true}` on the same socket answers with a `WindowTree` per
  window (title, content size, active, nodes in tree order: role, name,
  value, bounds in the content's logical pixels, enabled, clickable,
  focused, checked, selected), from what each window's next frame
  records (`agent::record(window, cx, Node {…})`: the kit's widgets do it
  for themselves; nothing is recorded otherwise). A program that isn't a
  kit app can answer for another process with `commands::serve_trees(pid,
  app_id, trees)` (the Olo extension's host does it for Chrome). Layer
  surfaces (Deskbar, the desktop) answer too, marked with their
  namespace (`layer`). `{"hints":[ID…]}` asks only about the surfaces
  named (their `wl_surface` protocol ids, which app_server knows them by)
  and answers as soon as those have drawn, each tree marked with its
  `surface` and cut by `WindowTree::pickable` to what takes clicks and
  what holds it (a window's text, thousands of nodes, goes): keyboard
  hints ask this way, so they never wait on a window nobody can see.
  (`serve_trees` can't tell its surfaces apart and answers it with all
  of them.) `{"path":ID}` (`commands::path(pid, surface)`) answers with
  the file or folder that surface's window shows (`agent::set_path`), or
  null. **Record any text you paint yourself** that an agent should be
  able to read or click (`style::label` does), and give a painted thing
  that takes clicks a role and name (`Node { role: "Button", name,
  clickable: true, … }`), so agents and keyboard hints can pick it.
  `{"focus":ID}` (`commands::focus(pid, id)`) gives the keyboard to the
  widget with that tree id. Blocks for up to two seconds.
* **What looks off, for agents** (`commands::review(pid)`, automatic):
  `{"review":true}` on the same socket runs the layout review (see
  "Layout review" under Testing) over the next frame of every window and
  answers with a `WindowReview` per window (title, content size, scale,
  findings: level, kind, what and where); `olo-screen review` and
  `tsession review` print them (`--all`: Deskbar and the other layer
  surfaces too, found by the processes on the session's display, since
  they have no window to find them by).
* **How a window keeps up** (automatic): `{"frames":true}` on the same
  socket answers, per window, the frames drawn since the last ask and
  the slowest and total time drawing them (`FrameTimes`, microseconds
  from taking input to handing the frame to the GPU), each frame's mark
  (when, how long, where its scroll views were: `Window::note_scroll`,
  which a ScrollView and a ColumnListView each call as they paint)
  and the GPU drawing them. The UI thread answers it, so how long the
  answer takes is how long the app would keep a person waiting
  (`tools/dev/perf-fuzz`).
* **⌘A in text fields** selects all of the field: a `TextField` with the
  keyboard takes ⌘A, ⌘C, ⌘X and ⌘V itself; otherwise they go to the
  menu's Select All, Copy, Cut and Paste.
* **Full screen**: `WindowOptions::new(..).fullscreen()` opens it full
  screen on the display the person is at, asked before its first frame
  (GPUI's `WindowBounds::Fullscreen`; app_server folds it into the first
  configure), for something that takes the whole of the person's
  attention and is done from the keyboard (the palm trainer).
* **Fixed-size windows**: `WindowOptions::new(..).min_size(w, h)` with
  `fixed` (preference panels, alerts); `max_size(w, h)` fixes a side
  where it's no larger than the smallest (a Find panel's height), and
  app_server neither tiles nor groups such a window.
* **Dialogs** (`WindowOptions::dialog()`; `alert(..)` for alerts): a
  window that belongs to another and should stay in front of it without
  being its size. The kit marks it with `xdg_dialog_v1` (alerts are also
  `set_modal`); app_server opens it over its parent before its first
  frame (kept on screen), never groups it into the parent's tabs and
  keeps any child above its parent when the parent is clicked, focused
  or raised (`wm.rs` `keep_children_above`). `child()`: a window over
  the app's window with the keyboard (a chat's Info), never a tab beside
  it. A service's dialog for another program (the screen-sharing picker)
  takes `.foreign_parent(handle)`, the `wayland:HANDLE` that program
  exported (xdg-foreign; a portal is given it as the parent window): the
  kit imports it and sets it as the window's parent, and app_server opens
  it over that window as it does over one of the app's own. The other
  way, `window.export_handle(done)` (Olo's GPUI) exports a window of
  ours and gives its handle (None without xdg-foreign), for a program we
  start to open over it: the file panel's look (`olo-viewer --look
  --over=HANDLE`). A window opened over its parent that then sizes
  itself (the look, fitted to its picture once loaded) is centred over
  it again, until it's moved or the person resizes it (app_server
  `keep_centred`, `WindowMeta::centred`).
* **A terminal's keys** (Terminal): a window that consumes the keyboard
  itself takes key events before the menus (its `key_down`), lets the
  menus' own shortcuts through (`menu::shortcut_command`) and shows
  them as pressed (`MenuBar::raw()`). With a command key app_server turns
  into Ctrl (⌘, or BeOS's Alt: the keyboard profile's, followed live by
  `olo_kit_base::keys::command_key`), app_server sends it to a terminal as
  its *right* Ctrl plus Shift and every physical Ctrl as the left one;
  `gpui::right_control_held()` says which, so ⌘ and Ctrl are told apart.
  Outside Olo, or with ⌘ off, ⌘ is any Ctrl.
* Background work goes on threads; post results with `Proxy::send` and
  take them in `message_handler` (then `cx.notify()` the view).
* Periodic refresh (clocks, graphs): a GPUI timer (`cx.spawn` with
  `timer(..)`), or `window.request_animation_frame()` for an animation;
  never to look for changes on disk: watch them (rule 7 above).
* Layer-shell surfaces (Deskbar, desktop): `WindowOptions::layer(namespace,
  layer, anchor)` and `layer_options(|o| …)` (its output, keyboard,
  exclusive zone, margins). One naming an output not known yet waits for
  it. `keyboard` is `None`, `OnDemand` (focus on click: the desktop) or
  `Exclusive` (every key while it is up: pickers such as Screenshot's
  region overlay; app_server gives it the keyboard on the commit it
  first shows with, and the clipboard's offer with it, so it can paste
  or read the clipboard at once). A click on a top or overlay layer that is `None`
  (Deskbar) leaves the keyboard where it was, never going through to
  what is under it; a menu of its closing hands the keyboard back to
  what had it as the menu opened.
* The pointer's shape: `window.set_cursor_style(style, &hitbox)` over a
  hitbox (`CursorStyle::None` hides it, as the screen saver does); shapes
  go to app_server by name over cursor-shape-v1, and app_server draws
  its own cursor for them; apps never ship cursor images.
* Staying awake: `window.set_keep_awake(true)` holds a zwp_idle_inhibit
  inhibitor on the window (no dimming, screen saver, lock or sleep while
  it is visible) until `false` or the window closes; app_server counts
  these in `PowerStatus::app_inhibitors`. The Viewer and Quick Look hold
  one while a video plays.
* Activation: `window.activation_token(done)` asks for an xdg-activation
  token from the latest input in the window, to hand to another program
  so its window may come forward; app_server honours a request only with
  fresh input from the asking app (or from the token's maker, when it has
  the keyboard: an app opening a link in another hands its click on), a
  click handed on by one of our programs, or a token it issued itself
  (`olo_protocol::launch_activation_token` asks for one,
  `Request::ActivationToken`, for launchers run without a click, unless
  one is in the environment; `issue_activation_token` asks whatever is
  there, as olo-chrome does: the opener's token in its environment may
  be long spent, and a Chrome running already is handed the new one).
* Scrolling: GPUI's `ScrollWheelEvent::kind` says whether a wheel's
  notches (eased in by the Wayland client, 48 points a notch), fingers
  on a touchpad or their glide (app_server's `continuous` scrolls) moved
  it, and `touch_phase` where in a scroll it is; fingers and the glide
  stretch past an end and spring back (`gpui::Elastic`, which every
  scroll view keeps, and a view that scrolls itself keeps its own:
  Terminal's scrollback, the editor). Keep any rounding to lines as a
  running remainder.
* The menu bar opens a menu on press, as on BeOS; pressing the open
  menu's title again closes it, but a menu that slid open as the pointer
  crossed the bar stays open for that press.
* Session lock (the lock screen only): `olo_ui::lock_session` and
  `unlock_session`. See "The lock screen".

### Drag and drop (`olo_ui::dnd`, GPUI's drops)

Over Wayland's data device, so it works within a window, between
Olo's apps and with any other Wayland program. (app_server tells a drag
where the pointer is as it starts, `dnd_requested`: an app asks once the
pointer has gone its drag distance, and in a quick drag the pointer is
already over where it goes, and still; until it moved again the drag
had no target, and was let go onto nothing.) Escape ends any drag,
nothing dropped: app_server's for a drag-and-drop (its source told it
was cancelled, the window under it that it left: `escape_drag`) and
for a window being moved (back where it was, zoomed again if it was,
an auto-tiled one in its tile), the kit's Frame for GPUI's drags in a
window (`stop_active_drag`); the Escape reaches nobody. As in BeOS (the Be
Book's `DragAndDrop.md`), a drag carries the same data in several types
and offers actions; where it is dropped picks the type and the action.

* **Dragging out:** as a press drags (the button still down),
  `dnd::start(window, cx, drag, done)` with an `OutgoingDrag`:
  `dnd::files(&paths, copy, move)` carries `text/uri-list` (file://
  URIs: file managers, browsers, editors) and the paths as plain text
  (terminals); add types only Olo knows first; `dnd::as_link(drag)` asks
  a folder for links instead of copies (`dnd::LINK_TYPE`);
  `dnd::picture(window, cx, icon, size, label, hotspot)` makes the
  picture under the pointer (the icon, its name below in the
  selection's colours). `done` gets the action it was dropped as (None:
  nothing took it). Viewer and Quick Look drag their file out from the
  picture when it fits (a drag that doesn't pan), Quick Look also from
  its footer's icon, Get Info from its icon. Tracker's folders are
  spring-loaded: a drag resting on one for 1 s opens it. Deskbar takes
  files dropped on a running application's row and opens them with it
  (`open::open_with(handler, paths)`: our apps get them all as
  arguments, a desktop entry at once with %F/%U or once each). Dragging
  a row out carries the application's program as a link (ours, or an
  entry whose command is a program alone); Tracker makes a link named
  as the application, and draws links to our programs with their icons
  (`Entry::app_icon`). Hit-test a drag where the button went down: by
  the time it's a drag the pointer has moved on.
* **Taking drops:** what's dragged over a window
  (`Window::drop_offer`: its types, what its source allows, whether one
  of this program's windows dragged it out, its files) is offered to its
  parts as they paint; one that would take it says so each frame
  (`window.accept_drop(target, mime, action)`; no call refuses) and
  shows it: an item highlights itself, a view or window outlines itself
  (2 px, keyboard-navigation colour). What's dropped comes to the
  window's `on_dropped` handler (`Dropped`: target, type, data, action,
  position; `paths()`, `text()`), its data read off the UI thread.
  What other programs drag: Chrome (156, on Wayland) drags a picture out
  of a page as `text/x-moz-url`, `text/html`, text and the picture itself
  as `application/octet-stream;name="sun.png"`, with no `text/uri-list`;
  a link as `text/x-moz-url`, `text/html` and text; a link's address and
  text read as UTF-8 from Chrome, UTF-16 from Firefox. Tracker takes, in
  order: files (`text/uri-list`), a file by its contents (saved under
  its name, made safe to save under), a link (a clipping of its address
  named by its text: "Docs link clipping.txt"), text (a clipping).
* **Text:** press on the selection in a text field and drag it out, as
  on BeOS; dropped back in the same field it moves, anywhere else it is
  copied (a folder makes a clipping). Text dragged over a field shows a
  caret where it would go and is put there, selected; files are left to
  the window. StyledEdit makes a move with `Doc::move_text`, so styles
  and undo follow.
* A secondary-button drag asks (as on BeOS): the place it lands shows a
  menu (Tracker: Move Here / Copy Here / Create Link Here).
* Headless screenshots show the drag picture; `tsession down X Y
  [right]`, several `move`s, then `up [right]` drives one.

Tracker selects as file managers do, in every view (`ListState`'s
methods, above): click, ⌘-click, ⇧-click and ⌘⇧-click; a plain click on
one of several selected leaves it alone (a drag from it takes them
all, and a ⌘ or ⇧ drag from one not selected adds it first); the
arrows (in the icon views, across and a row of icons up and down),
Page Up and Down, Home and End, each with ⇧ to extend; typing selects;
⌘A and Invert Selection (Edit menu); a band from empty space in every
view, with ⇧ adding and ⌘ flipping, scrolling on past the edges (the
icon views lay out only the rows it crosses); Enter or a double-click
on one of several opens them all; Open Parent (⌘↑) comes back with the
folder it was in selected. The desktop, whose icons are anywhere and
have no order, takes ⌘ and ⇧ alike as add or take away. A listing
that changes keeps the selection, anchor and cursor by path.
Folders are listed on workers, so a selection made in code (Show in
Tracker, a new or renamed item, the selection put back after a reload:
`FolderWin::select_paths`) may name items not listed yet: those wait
for their own folder's listing (`pending_select`, by folder) and are
given up once it lands without them, never touching another column's
selection. In column view, `select_paths` on items deeper than the
window's folder opens the columns down to theirs, each selecting the
folder the next one shows, so a window comes back from a reload with
its columns. What such a selection selects scrolls into view, with
some of what's around it, in every view (`FolderWin::show`), and a
change of view keeps the selection in sight. → into a column not
listed yet selects its first item when it lands (`Pane::first`), as it
would have had the listing been there.

Tracker (`view.rs`: `Drag`, `DropTarget`, `take_drop`, `item_drop`):
files drag from every view and the desktop and drop onto folders (in a
view or on the desktop), into a folder window (anywhere in it; in the
column view, a column or past the last one), onto the Trash (moves them
there) or into another app. Within one file system they move, across
file systems they copy, ⌥ copies anyway; Tracker knows which before the
drop from `application/x-olo-device-N` in its own drags, and only its
own drags are moved (another program might delete what it was told was
moved). Text dropped on a folder becomes a clipping file named after
its first words. The desktop keeps icons where they are dropped
(`~/.config/olo/desktop.json`, `icons`: name or path → offset of the
cell from the screen's top-right, in logical px; things dropped onto it
from elsewhere land where they fell; a rename carries the place over);
the rest fill the grid around them, and Clean Up in its menu puts every
icon back on the grid. Terminal types dropped files as their
shell-quoted paths (and text as if pasted), bracketed when the program
asked for it: drag an image into Claude Code there.

### Ad-hoc popup menus (`menu::context_menu`, `menu::popup`)

A context menu is `menu::context_menu(window, cx, menu, position,
on_choose)`; a menu under a button `menu::popup(window, cx, menu,
anchor, corner, on_choose)` (`MenuField`'s is one). `on_choose` gets the
chosen item's command: an app's own context menus use its menu bar's
command names and send them where the bar's go.

A menu taller than its display is as tall as the display allows (24 pt
off its top and bottom, a whole number of items and one arrow, so
neither end shows half an item) and scrolls, as BeOS's did: an arrow at
each end there's more past, which scrolls it (480 pt/s, a tick every
8 ms only while the pointer rests there and there's more) with the item
under the pointer lit as they pass; the wheel scrolls it, and ↑↓ bring
the item chosen into view. A pop-up field's menu opens scrolled to the
item chosen, over the field. Agents read every item, shown or not, and
the arrows (Scroll Up, Scroll Down).

Every item with keys shows them, right-aligned, as the keyboard
profile's platform writes them (on a Mac in Mac order, "⌘O", "⇧⌘T",
"⌃⌘H"; on Windows "Ctrl+O"; on BeOS "Alt+O"), in context menus as in the
bar. Which keys an item has comes from the profile, by its command (see
"Keyboard profiles"): an app gives the Mac's (`Menu::item(…).key("o")`,
`.shift_key("t")`), and the menu bar gives each item the keys its
command has in `olo_protocol::app_keys` for the profile
(`menu::resolve`). A context menu gives each item without keys the
keys its command has in the window's menu bar, resolved
(`Menu::keys_from(&bar)` after `menu::resolve(&mut bar, raw)`), so a
context menu's Open shows ⌘O without the app saying so twice; keep
context menu commands the bar's. Keys that
aren't a ⌘ shortcut of the window's own (a desktop shortcut from
`olo_protocol::shortcuts`, a plain key such as Space for Quick Look)
are named with `.keys("⌃⌘H")` after the item (`Action::keys`); they are
only shown, whoever gets the keys acts on them, and shown in place of
the item's own shortcut if it has one. `Action::key_label()` is what's
shown, and agents read it as the item's `value`. A shortcut on = is
pressed with + too (⇧= types +), as ⌘+ zooms in on a Mac.

In a list of windows, `.minimized(true)` after an item marks a minimized
one: a hollow diamond where the check mark goes and a lighter label
(`style::diamond`), still enabled (choosing it brings the window back);
agents read `minimized` on its `MenuItem` node.

### Tips past the window's edge (`olo_ui::tip`)

GPUI's tooltip (`widgets::tooltip(text)` on an element's `.tooltip(..)`)
is drawn inside the window, so in a window smaller than what it says it
is cut at the edge. `tip::hover(window, cx, key, text, anchor, hovered)`
shows it in a popup of its own instead (`WindowKind::Tip`: an xdg_popup
with no grab and an empty input region, so it takes no clicks), below
the thing or else above it, slid to stay on the screen; it comes after
a rest, at once if another showed a moment ago, and not after a click
(`tip::pressed`). Use it where the tip can be wider or taller than the
window: Deskbar's tray, Terminal's sudo window's command. Elsewhere
`widgets::tooltip` is fine.

### Text cut short (`olo_ui::cut`)

Text that doesn't show in full shows whole in the kit's tip (`tip`)
when the pointer rests on it, at once while ⌘ (Super) is held (⌘ as
apps get it: the right Ctrl with a profile that has a command key,
`cut::command_held`, as Terminal tells it). Every kit window does it:
its `Frame` follows the pointer, a press puts the tip away until the
pointer leaves the text. What the kit cuts is noted as it's painted:
`style::label_fit`, `label_runs_fit` and `label_on_baseline_fit` (list
cells, tab and strip titles, pop-up menu fields, sidebars, dashboards,
thumbnails), and a label running past the side of what it's painted in
(`label`, `label_runs`; a row half scrolled out of view, top or bottom,
isn't). GPUI's own text elements (a `div().child(text)`, `StyledText`)
note themselves the same way (`gpui::CutText`, the record `cut::note`
makes too): truncated (`truncate()`, `text_ellipsis()`), clamped short
of its rows (`line_clamp`: the sudo sheet's command), or one line
running past a side of its clip (a paragraph's long word isn't). An app
that cuts text itself says so with `cut::note(window, full, bounds)`
(Tracker's icon names, Telegram's attachment chips and bubble lines). The notes are paint records, so
a cached view's replay keeps them; the pointer's moves look through
them once after each frame drawn, then through the few rectangles cut
(0.3–1.9 µs a move in Tracker and Activity Monitor). Agents read cut
text whole: in a window's tree a node named by what shows of it
("Quarterly rep…") has its whole name, and a cut text no node names is
a `Label` of its own (`agent::whole_name`, when the tree is asked for).

### Showing a control (`olo_ui::spotlight`)

`spotlight::show(handle, tab, group, name, cx)` says "here" about one
control, as macOS's System Settings lights what its search found: the
tab called `tab` is clicked if it isn't the one showing, then the
control called `name` (a label's colon aside) is found in the window's
agent tree, in the innermost `Group` called `group` if there is one
(else any so named: a box's name can be the device's), and lit with a
ring in the keyboard-navigation colour, two lines wide on device
pixels, over a faint wash. It comes in 150 ms, holds and goes over the
last 450 of `spotlight::LIT` (1.6 s); the window's `Frame` draws it
over everything else. Frames are asked for only while it comes and
goes: held, one timer wakes the window for its going, and once gone
nothing runs. False if there's no such control. Preferences uses it
for `--show` (the command palette's settings); an agent can use it to
show the person what it's talking about.

### What every app does the same way

The widgets are listed under "The GPUI kit" below; this is how they
behave, so that something an app draws itself behaves the same.

The pointer leaving a window un-hovers everything in it. Olo's GPUI
takes the pointer out of the window (its hit test empty, the window
drawn again) and, no button held, gives the window a move to nowhere,
so state kept from the pointer's moves goes too (a graph's readout, a
key lit under it: before, they stayed lit after the pointer left from
there). A div's `on_mouse_move` is called only while the pointer is
over it: state an app keeps from it (DeskCalc's key under the pointer)
is cleared in `on_hover(false)` (an `id` makes the div able to say),
and a canvas's hover drawn only while its hitbox `is_hovered`.

An app's own files (its settings in the config folder, what it
remembers in the state folder) go through `store` (`olo_kit_base::store`,
olo-ui's `store`): `load(path)` is the JSON there as a `T`, or None (no
file, or not a `T`: the app's defaults); `save(path, &value)` writes it,
laid out to read, off the caller's thread, one file after another in
the order saved, so of quick saves to a file only the last is written;
`save_bytes` for JSON an app lays out itself (Notifications' long
history, compact); `write(path, bytes)` on the calling thread (a helper
that's about to exit, a file kept "now" before an upgrade). Every write
goes beside the file (`.NAME.saving`, which backups leave out) and is
renamed over it, so neither a reader nor a crash halfway through sees
half a file; a new file is the person's alone (0600), one there already
keeps its permissions (AirPods' keys stay private, Claude Code's
settings stay as they were). The kit writes what's still to be written
as the app quits (`on_app_quit`) and before an upgrade re-execs it
(`store::flush`), so a setting changed just before the last window
closes isn't lost.

A window an app keeps one of (its main window, History, a review) is
shown with `open_or_raise(kept, cx, options, build, keep)`: brought
forward if it's open, else opened, `keep` handed the one showing; done
once the calling update is over, since a window can't be brought
forward while it's being updated (it would look closed, and open
twice). `raise(handle, cx)` alone brings one forward if it's still
open, as an app launched again does (a second launch hands itself to
the first, `instance`, which raises its window).

A window whose content needs a height a short screen may not have says
so: `WindowOptions::content_height(h)`; in a window less tall than that,
its `Frame` lays the content out at `h` all the same and scrolls it, the
bar beside it (a Preferences panel on a small display: Preferences sizes
the window to the screen, wider by a scroll bar, and the Frame scrolls).

A window that should open at the size it was last left at says so:
`WindowOptions::keep_size("main")` (a name among the app's windows;
`size` is its first). Its size is noted as it changes, in memory (a
live resize writes nothing), and the app's sizes written when one of its
windows closes or it quits (`kept.rs`, `store`, one file an app in
`~/.local/state/olo/windows/`); the next opens at it, held to its min
and max. A size the compositor chose (a tile, zoomed, full screen,
GPUI's `Window::tiling`) isn't the person's and isn't kept. The layout
lint ignores kept sizes. Kept: Activity Monitor (and its inspectors),
Logs, Disk Usage, Notifications' History, Passwords, Telegram,
StyledEdit's documents, Token Pool, DriveSetup, Doctor, Agent Screens.

Closing (or reverting, or moving on from) something with changes not
saved asks the same question everywhere: `widgets::ask_to_save(cx, app,
name, "closing", then)`, "Save changes to “name” before closing?" with
Cancel, Don't Save and Save (the default), as BeOS's StyledEdit asked;
`then` hears `Some(Unsaved::Save)`, `Some(Unsaved::DontSave)`, or None
(Cancel, Escape, the alert closed: carry on). StyledEdit and Viewer ask
it; Passwords' item editor asks to discard instead (its Save is the
item's own, in the editor).

What a view has to do at some time goes through `due` (olo-ui), so
nothing runs while nothing is due: `Due` is one timer for the next
thing, set to when that is (`at(when, cx, then)`: the timer there is
kept if it's for the same time; None, no timer) or kept to the soonest
asked (`by(when, cx)`, which draws the view again then and lets its
render say what's next); `Notice` is a message that goes of itself
(`say(text, for)`, `text()` while it lasts, `until()` for the view's
`Due`): Passwords' status line ("Deleted “Bank”.", its auto-lock and
sync on the same `Due`), Telegram's problem banner.

Text fields behave the same everywhere: a click places the caret, a
double click selects a word, a triple click the line, and a right press
opens Cut / Copy / Paste / Select All, keeping the selection it landed
on. `TextField`, `TextControl`, `TextView` and the editor do this
themselves; nothing that takes clicks may sit over one.

Lists select as a file manager does (`olo_kit_base::list::ListState`,
which the column list and Tracker's icon view share): `click(row,
mods)` alone; ⌘ adds or takes away; ⇧ everything from the `anchor`,
which stays, so another ⇧-click makes the range again from there; ⌘⇧
that range added. `move_to(row, extend)` is the keyboard's (⇧ grows or
shrinks the range from the anchor; `cursor` is where the keyboard is),
and `band(mods)` a selection band's (⇧ adds what it covers, ⌘ flips
it; near the edges it scrolls on). The keys: the arrows, Page Up and
Down, Home and End (⇧ extends), Return opens; ⌘ and Alt with them are
left to the menus. Typing goes to the row whose name starts with what's
typed (`TypeSelect`: within `TYPE_SELECT_PAUSE`, 0.9 s; else the first
name after it; one letter again and again steps through the names
starting with it; a space continues a name but starts nothing, so Space
is Quick Look only while no name is being typed). An empty list says
why (`empty_text("No lines match.")`).

Menus are real `xdg_popup`s drawn by the kit. A submenu with nothing
enabled in it is drawn disabled and doesn't open, as a BMenu disabled
whole; its shortcuts and agents' `invoke` do nothing. Split very long
lists into submenus (StyledEdit's font families). app_server hands the
keyboard back to the window as soon as its last popup is destroyed
(ending Smithay's popup grab first, which ignores a focus change until
the popup's surface has gone too; and, the popup gone, its window found
from what had the keyboard when the grab began), so Escape or an item
chosen leaves the window typing again at once.

Alerts: the icons are BeOS's `warning`, `stop`, `info` and `idea`; the
last button is the default and Escape picks the first. The kit makes an
alert a child (`xdg_toplevel.set_parent`) of the app's window that has
the keyboard, and app_server opens any window with a parent over it as
BAlert does: centred across, a quarter of the way down. An alert with no
parent (a modal xdg-dialog naming none: asked from the desktop, a menu
or Deskbar, where no window of its app had the focus, or a toolkit's
parentless message box) opens in the middle of the display the pointer
is on, kept off Deskbar's column, in front and focused (`place_alert`),
and stays over all its app's windows until it's answered
(`keep_children_above`, BAlert's app-modal feel). A fixed window (min
size = max size) is always the size it asks for: app_server doesn't
give it a size remembered from before a restart.

File panels (`olo_ui::file_panel`) are BFilePanel: a path pop-up that
also goes to the standard folders, Favorites (the folder shown added or
taken away) and Recent Folders, all from `olo_ui::places`
(`~/.config/olo/places.json`, which Tracker's Go menu shares, both
adding the folders they go to); Home; a Name/Kind/Size/Modified list as
Tracker's, folders first and names in reading order, following the
folder as it changes; a "Save as:" field, and a question before
replacing. Space takes a look at the selected file in the Viewer
(`olo-viewer --look --follow --over=HANDLE FILE`: a process of its own,
centred over the panel by the handle the panel's window is exported as,
put away by Space or Escape there, by Space again in the panel, and with
the panel). It follows the panel's list, as Quick Look does: its arrows
(caught before what it shows scrolls with them) ask the panel for the
file before or after (`step -1`/`step 1` on its stdout, read on a
thread and handed to the panel on a channel), the panel moves its
selection there (past folders) and tells it the path to show (a line on
its stdin), as it does when its selection moves by itself. An
item's menu (a secondary click) has Quick Look, Open With ▸
(`olo_ui::open_with`, the folder's kinds warmed as it's listed) and
Show in Tracker (`olo_kit_base::tracker::open`). A
filter is a glob, ignoring case, or a MIME type, a kind of one counting
(`olo_xdg::mime::is_a`); which filters each file fits is worked out
with the listing, off the UI thread, so switching is instant.
`olo_xdg::mime::from_name` finds a name's type from shared-mime-info's
globs, indexed by suffix (most are `*.ext`), the few other patterns
made into characters once, so a big folder is a few lookups a file
(`/usr/bin`, 2,652 names: 1.5 ms); the person's own kinds
(`mime::own_kinds`, `~/.config/olo/kinds.list`, `name|ext;ext|type`,
looked at at most once a second) come first. Open and folder panels
give the list the keyboard.

Pictures are read by `olo_images::open(path)` (`crates/images`, which
app_server uses too): everything the `image` crate reads plus JPEG XL
(jxl-oxide), AVIF (avif-parse and rav1d built without asm, with our own
YUV to RGB and alpha) and HEIC (heifer), EXIF-rotated, with a 1 GiB
allocation limit; it returns the image and its format's name. Its
`av1`, `vp9` and `hevc` modules decode video for olo-preview. HEVC is
rust_h265 (MIT/Apache, pure Rust), chosen over rusty_h265 (2026-11-03):
both bit-exact on 35 JCT-VC conformance streams (8 and 10 bit, tiles,
WPP, dependent slices, unequal bit depths) and identical on two real
1080p clips, rust_h265 faster alone (104 and 87 pictures a second
against 93 and 68) and with threads (250 and 341 at four), and reading
the range extensions too. Keep its
dependencies permissive: zenavif / rav1d-safe were dropped for being
AGPL. `OLO_SAMPLES=dir cargo test -p olo-images -- --ignored` decodes
example.avif and example.heic from `dir`. Colour emoji are GPUI's text
system's, drawn with the text; `olo-emoji` (`crates/emoji`) finds and
paints clusters itself where that's wanted (Telegram's messages of
emoji alone, shown big): it shapes with harfrust and paints COLRv0/v1
with tiny-skia (Noto Color Emoji is COLRv1); render test images with
`EMOJI_OUT=/var/tmp/dir cargo test -p olo-emoji`. Which characters
start an emoji (`starts_emoji`: pictographs, the symbols shown as emoji
by default such as ✋ ⚡ ✨, and any asked to be one with U+FE0F, a
keycap or, for ☝ ⛹ ✌ ✍, a skin tone) is the same everywhere: GPUI's
text system takes it from `olo-emoji` without its drawing (`default-
features = false`; the `render` feature is skrifa, harfrust and
tiny-skia). GPUI keeps the colour glyphs it draws for every window of
the app (they cost ~60 µs each to draw; up to 32 MB of them, those for
other scales let go first; `TextSystem::forget_kept_glyphs` lets go),
where an outline glyph goes to the atlas that asked.

Toolbars are `metrics::TOOLBAR_HEIGHT` tall (a row of controls, `TIGHT`
above and below), with their line under them (`style::hline_above` at
the bottom); what follows starts a line lower (`style::hairline`), so at
1.5× it is a pixel down, not a point and a half, and the controls stay
centred. App settings live in `olo_ui::config_dir()`
(`$XDG_CONFIG_HOME/olo/NAME.json`); user folders come from
`olo_xdg::user_dir("PICTURES")` (user-dirs.dirs, None if unset or
missing). Bytes too many to keep in memory (a download being gathered)
go in `olo_xdg::scratch_file()`: a nameless file under
`olo_xdg::cache_dir()`, on disk rather than /tmp, gone once closed.

Cards of different sizes (Notifications' history) are placed by
`olo_kit_base::masonry`: `masonry::columns(width, min, gap)` fits
columns of at least `min`; each item gives (span, height at that width)
and `masonry::arrange(cols, column, gap, &items)` places them in order,
each where the columns are lowest, filling the hole a wide one would
leave under its shorter column with the next small items that fit
(looking four ahead, so the order hardly changes); it returns their
places and the height of the lot. `masonry::neighbour(&places, i,
Step::Down)` is where an arrow key goes from item `i`, by where things
are. A card is `style::card(window, r, palette, hover, selected)`:
rounded by `metrics::CARD_RADIUS`, a hairline edge and a soft shadow,
its content `metrics::CARD_INSET` in; selected, it lies in the
selection's colours on a deeper shadow (picked up, not outlined). Its
colours are `style::card_ink(palette)` (`CardInk`: `gutter` between
cards, `card`, `edge`, `hover`, `text`, `dim`, and `well` for something
set into a card, a message's balloon; `dim` and `well` are the text
colour, see-through, so they read on a selected card too). On dark the
card is a lift above the paper, not a darker hole in it. Don't head a
card with a coloured band or a little yellow tab: a card is not a
window, and a tab on every one repeats the window's own.

### Searching and filtering (`olo_ui::search`)

Every app's search is the same language, field and filter bar. The
language, its `Facet`s and `suggest` are `olo_kit_base::search`: words
(each must match, any order, ignoring case), `"phrases"`, `-word` to
leave out, and the app's filters as `key:value`, `key:a,b` for either,
`-key:value`, `key:"two words"`; a `key:` the app doesn't know is a word
(`https://…` finds itself), a part still being typed (`key:`, `-`) asks
nothing. `Query::parse(text, &search::keys(&facets))`, then
`query.matches(|key, needle| …)`: the app says whether an item has the
needle under the key (`None`: anywhere in its text); `Needle` matches
without case, memchr-fast for ASCII (`in_text`, `is`, `starts`, `find`,
`find_all`). A value being typed is best matched by its start
(`starts`), so the list narrows as it's typed. `query.score(&[(field,
weight)])` ranks (whole field 8, its start 4, a word's start 2,
anywhere 1, times the weight) to put the best first.

The field: a magnifier, the text with filters' keys in blue and
left-out parts in red, a clear button, and while typing a list under it
of the filters and values that fit the word at the caret: ↑↓ choose,
Tab takes the chosen or first, Return the chosen, Escape closes the
list, then clears, then lets go. The filter bar: a pill per facet
(`Facet::typed_only` keeps one out), "Label: Any ▾" or what's chosen
(bold, tinted), whose menu has Any and each value with its count where
a shortcut would be; choosing one rewrites the query's text
(`search::set_filter`), so the field, the bar and agents typing a query
always agree. Facets are built from the data (values and counts) and
kept until it changes, not made every frame.

In olo-ui: `search_field(cx, hint)` makes an `Entity<SearchField>`
(coloured filters through `TextField::set_colors`, suggestions drawn
over what's below with `deferred`); it says `SearchEvent::Changed` and
`Entered`; `set_facets(Rc<Vec<Facet>>)` (drawn again only when the
filters or their values change, not their counts, which change with
every new list), `set_text` (no event), `choose` (set, and said as
`Changed`, as typing it would be: a bar's choice), `focus` (⌘F,
its text selected). Tab and Return take suggestions by capturing the
field's own actions (`FocusNext`, the field's `Enter`): bindings run
before key listeners. `FilterBar::new(id, text, facets).max_width(w)
.on_change(|text, window, cx| …)` is the bar as an element
(`FilterBar::width(cx, text, facets)`, about: each menu is sized from
its text as shaped in the window, and a pixel more, so rounding to
device pixels never cuts it short); its menus are kit popups.
`.drives(&field)` has its choices go into the field (`choose`), so an
app follows the field alone, one subscription for typed and chosen.
`search::header(&field, id, facets, width)` is the search over a list
as Log Viewer and Replay have it: the field and the bar in one row when
the field keeps `min_field` points (240 unless said), else the bar
under it, `indent(search::list_indent(window))` putting its first
menu's text over the list's first column's; `count("12 of 340")` at
the end of the field's row; `no_clear()`. Passwords' bar, over its list
apart from its toolbar's field, `drives` it too.
`search::runs(text, font, color, spans, marks, mark)` makes text runs
from colour spans and marks (marks are runs' backgrounds), and
`search::highlight(text, &query, font, color, mark)` marks what a query
found (not on a selected row's colours).

### The text editor (`olo_ui::editor`)

For documents (StyledEdit's window; anything a person writes at length)
use the editor, BTextView, not a multi-line `TextField`: it lays out
each paragraph on its own, only while it shows, so a long document
keeps to 120 frames a second (the API is under "The GPUI kit").

A paragraph is set by `Document::paragraph` (`editor::Paragraph`:
`left`, `first` and `right` indents, space `above` and `below`, its own
`align` and `justify`, a list's `marker`, drawn before its first row in
the room `left` leaves, in the paragraph's first style; its `tabs`,
from the text's left edge, left, centre, right or decimal stops, then
one every so many points, or with none a stop every four spaces' width,
as code editors have them). Each paragraph wraps within its indents;
↑/↓ keep to an x from the text's edge, so rows line up across
paragraphs indented differently. Pictures go in the text as characters,
`editor::PICTURE` (U+FFFC), laid out as big as the picture and named in
`Paragraph::pictures`; each is drawn over its character (a frame while
it loads), tinted when selected.

A paragraph can be a table's row (`Paragraph::row`, an `editor::Row`:
each column's share of the width, and whether it is a header): its text
is its cells, a tab between each (`editor::cells`), or where its
document says they are (`Row::cells`: a Markdown row's, between its
pipes, the pipes in no cell; Tab goes cell to cell, and the document
adds a row on Return, `Document::types`; `Row::aligns` a column's own
alignment, a Markdown `:---:`), each laid out in its
column, `CELL_INSET` in from the rules, rows meeting with no space
between; a table is rows one after another. Hit-testing, the caret, ↑/↓
(into the cell under the same x in the row above or below), Home/End,
selection and spelling marks all go by cell. Rules are a device pixel on
device pixels (the palette's `control_border`), a header shaded
`control_background`. Tab and ⇧Tab go from cell to cell (selecting it;
Tab in a table's last cell adds a row), Return adds a row below, and an
edit that touches a row keeps its structure: what is typed or pasted
into a cell is one line, and the tabs and newlines a deletion takes are
put back after it, so cells and rows empty rather than join (Backspace
at a cell's start just goes back over its tab). A table's inner rules
can be taken hold of (a few points either side; the pointer a column
resize): dragged, a line shows where the rule would go, and let go,
`Document::set_columns` gets the row and its columns' widths (StyledEdit
sets every row of the table, one step to undo).

Keys are a Mac's (⌥ or ⌘ with arrows by word or to either end; Home/End
along the row; Page Up/Down; ⇧ extends). A double click selects a word,
a triple click the paragraph. A press on the selection drags its text;
text dropped on it goes in where it lands, and moves within the editor.
Dragging a selection past an edge scrolls. A right press outside the
selection puts the caret where it was, so the app's menu can be about
that word. Agents read it as a `TextInput` with the whole text
(read-only when it isn't editable).

A span can be `hidden` (no room, not drawn, the caret going past it as
if it weren't there: a Markdown mark off the caret's line; a row of
nothing but hidden text is nothing high) and have a `background` (filled
behind its text, a little wider than it and as high as its face: inline
code). A hidden span with a `picture` is its picture at its first
character, whatever that is (`![alt](file)` shown as the picture). A
paragraph can have a `shade` (filled from its indent to the text's edge,
top to bottom with its space above and below, so paragraphs one after
another make one box: a code block's ground), a `bar` down its left in
the room `left` leaves (a quotation's), a `rule` across its middle (a
Markdown `---`), and be `verbatim` (code: not checked for spelling).
Shades and bars of paragraphs one after another, alike, are painted as
one fill, under everything (a code block's ground is one box, whatever
its lines' heights at a scale).
`Options::line_numbers` (a size) numbers each paragraph down a gutter
at the left in the monospaced face, the caret's brighter, each on its
paragraph's first baseline; the gutter is as wide as the most digits
and the text starts after it (the right margin as it was), and it stays
put when the text scrolls sideways. `Document::types` says what Return
and Tab type, if not what they would (the bytes replaced, the text, and
where the caret goes in it): a Markdown list's next marker, code's
indent.

Spelling (`olo_kit_base::spell`): the system's Hunspell dictionaries
(`/usr/share/hunspell`, `~/.local/share/hunspell`) in the language of
`LANG` (else another of that language, else `en_US`), read by spellbook
on a worker the first time `olo_ui::spell::checker(cx)` is asked (None
until then; the windows draw again when it's ready). `Checker::check`,
`misspelled(text)`, `suggest(word)` (eight at most), `learn(word)`
(every app's, in `~/.config/olo/words`) and `ignore(word)` (until the
app quits). `spell::words` finds the words to check: letters with
apostrophes inside, leaving alone tokens that are addresses, paths,
numbers or identifiers and words in capitals; `spell::word_at(text,
at)` the one at the caret. Misspelled words get a red wave under them
(the palette's `failure`), all but the word at the caret while typing.
The editor checks a paragraph that changed on a worker and, when the
words come, draws its host's view again (`App::notify`), not the window:
a window refresh draws every cached view afresh too (StyledEdit's tool
bars, on every key's second frame).

### Source code (`olo_ui::code`) and Markdown (StyledEdit)

Code is shown and edited in the same editor: `code::Code` keeps a
text's `olo_syntax::Highlighter` (crates/syntax) and makes each line's
spans, the monospaced face at its size, coloured from the palette's
token classes in light, dark and night (`Palette::syntax`). The app
gives every change to `Code::edit` (StyledEdit's `Doc` keeps them with
the lines they're on: `Doc::keep_edits`, `take_edits`) and calls
`code::parse(cx, code, text, find)` when it draws: the parse runs on the
background executor, one at a time (again when the text changed
meanwhile), and the view draws again when the colours come. Until the
first is back the text is plain; after an edit, colours stay on their
characters (the old tree, edited) until the new one. A line's spans are
made once, when it first shows, and kept until the text or the tree
changes (only the lines a parse changed are made again). Nothing runs
while idle; nothing parses on the UI thread.

StyledEdit opens a file by its kind (`Mode`): code (`code::detect`: its
name, extension or `#!` line) in the code view with line numbers
(Document ▸ Line Numbers), Return keeping the line's indent and Tab
typing the file's own (a tab, or two or four spaces, as its lines are
indented; ⇧Tab takes one away); Markdown (`.md` and kin) as it looks
or as its source (Document ▸ Show Source, ⌘/); anything else, or a
file with styles of its own, as a styled document. Document ▸ Language
shows plain text as any of them. Code and Markdown are saved as their
text alone, so what's saved is what was written.

Markdown (styled-edit markdown.rs): the text in the editor is the
file's own Markdown, parsed again as it changes (pulldown-cmark, each
event with its place) into each line's block (heading, list item with
its depth, quotation, code block and fence, table row, front matter,
HTML, rule, a line in no block such as a link's definition) and each
byte's styles and whether it is content or a mark. As it looks, a
paragraph is set as its block is: headings larger and bold with room
above, a list item's bullet (•, ◦, ▪ by depth), number or task box
(☐ ☑) hanging in its indent, a quotation's bar and grey text, a code
block's shaded ground with its fence's language as a small label (code
coloured by that language: `olo_ui::code`, parsed on the spot as it's
small, kept while unchanged), a rule for `---`; inline emphasis,
strong, struck, `code` on its ground, links in the link colour; and
the marks that make them hidden, except on the lines the selection is
on, where they show dimmed, as typed (`Look::set_reveal`). A table is
a ruled grid (the editor's rows, `Row::cells` its cells between the
pipes, columns as wide as their longest cell and aligned as the
`|:---:|` line says, that line hidden off the caret): typed in cell by cell, Tab to the next, Return a new
row. HTML and link definitions show as typed in the monospaced face. Pictures (`![alt](file)`, a file beside the document, or one on
the web, `![alt](https://…)`: fetched once on the worker that decodes it,
at most 40 MB, into `~/.cache/olo/styled-edit/pictures` under its
address's SHA-256, those not fetched for a month going as new ones come;
its alt text until it's in) show as the picture off the caret's line.
A link opens with the command key and a click (the pointer a hand
over it while the key is down) or its menu's Open Link: a file from
the document's folder, an address with the person's app for it. As source: the same text in the
monospaced face, headings, links, code and marks each coloured, with
line numbers; the place (caret, selection, view) is the same across
the switch, as the text is. Printed, or exported other than as
Markdown, a Markdown document is set as it looks (`convert::
styled_copy`: `from_markdown_in`, its pictures from its folder) and
code in the monospaced face coloured by its syntax (the light look's
colours, parsed on the print's worker); exported as Markdown it's its
own text. Up to 256 KB a document is parsed again at
each change (about 9 ms a megabyte); a bigger one on a worker
(`markdown::parse`, once typing pauses for `code::PAUSE`), its lines
following each edit meanwhile (`Look::shift`: a typed byte styled as
the one before it), and only the lines the parse sets otherwise are
laid out again. Source code over `code::BIG` is parsed once typing
pauses too, and an undo step of text alone (code, Markdown) doesn't
copy every paragraph's settings. Return on a list item types the next one's
marker (a number one more, a task unchecked), in an empty one ends the
list; in a quotation, `> ` again. Font ▸ Bold, Italic, Strikeout, Code
(⌘E) and Link (⌘K) put the marks around the selection (or take them
away); Paragraph makes the selection's lines body text, a heading 1–6,
a quotation or a bulleted, numbered or task list, by their marks.

### Look and feel rules

1. **Colours only from the palette** (`theme::palette(cx)`):
   `panel_background`, `document_background`, `control_*`, `menu_*`,
   `list_*`, `shine`, `keyboard_navigation`, … Use
   `style::shade(&p, rgba, tint)` for bevel shades (it compensates in dark
   mode). Never hard-code a grey: it will be wrong in the other theme.
   Semantic accent colours (graph lines, file-type tints) must be checked in
   both themes.
2. **Spacing only from `olo_ui::metrics`** (`SPACING` 12, `TIGHT` 6,
   `STACK` 10, `WINDOW_INSET` 16, `BOX_INSET` 14, `LABEL_GAP` 10,
   `MARK_SIZE` 12 and `MARK_GAP` 6, `CONTROL_HEIGHT` 24, `LIST_ROW` 20,
   …). If you need a new kind of gap, add a named constant there rather
   than a literal in your app. Things line up as in BeOS: a box's
   contents with its label (both `BOX_INSET` in: BeOS's BBox drew its
   label 10 px in, and its preferences put what it held 10 px in), and
   a check box or radio button with what's around it by its mark, its
   label where the mark puts it (BeOS's 20 px in; Olo's 18), so a check
   box under a box lines up with the box's frame, not its text.
3. **Text**: draw single lines with `style::label(window, cx, x,
   center_y, align, text, size, font, color)`; it centres capitals
   optically and puts the baseline on a device pixel. Any line that can
   be longer than its room (a title, a path, a name the person typed)
   goes through `style::label_fit(…, max_width)` instead, which ends it
   with "…" rather than clipping a letter in half or running into its
   neighbour (agents still read the whole text). Counts people read have
   their thousands separated, `style::thousands(n)` ("30,001 actions").
   Fonts: `style::ui_font(weight)` and `style::mono_font(weight)`, at
   `PLAIN_SIZE` (12), `DETAIL_SIZE` (11: a subtitle, a caption, a chip)
   or `SMALL_SIZE` (10.5: graphs' labels, headings in capitals); a
   full-screen view's title at `TITLE_SIZE` (24) and its lead text at
   `LEAD_SIZE` (15), each line in `style::line_row(size)` (its line
   height up to the 2-point grid). The
   faces are Noto, shipped with Olo in `data/fonts` (see its README: Noto
   Sans and Noto Sans Mono, variable, with Symbols, Symbols 2 and Math
   for what they lack; OFL) and installed to `share/olo/fonts`
   (`install-session.sh`, and an upgrade's `sync_fonts`);
   `olo_xdg::fonts_dir()` finds them, and GPUI's text system and
   app_server (`text.rs`) both use them ahead of the system's, so Olo
   looks the same on any system. `olo_theme::fonts` lists the faces in
   the order they are tried (`UI`, `MONO`, `SYMBOLS`; the system's DejaVu
   Sans, CJK and emoji after). `style::CAP_HEIGHT` is Noto Sans's;
   app_server centres its own text (tab titles, Twitcher, hint badges)
   with `text::top_for_centre`, from the face's own metrics. Keys as on a
   Mac menu ("⇧⌘←"): `style::key_combo(window, cx, right_center, keys,
   color)`, each symbol on the text's baseline though they come from a
   fallback font. A failure beside text (an error's title, a failed row)
   leads with `style::stop_mark(window, r, palette)`, a crisp vector stop
   sign (12 px reads where an alert's icon would be mud). A pull request
   is GitHub's mark, `style::pull_request_mark(window, r,
   PullRequestShape::{Open, Draft, Merged, Closed}, color, node)`, 16×16,
   its lines whole device pixels at every scale (its commits a whole
   pixel's ring at 1×), `node` filling the branch's commit (its checks'
   colour). Say what a thing is with its icon, a chip or a mark, not a
   coloured band or edge along it: those clash with icons and look
   unfinished.
4. **Selection looks the same everywhere**: the palette's list selection
   (`list_selected_background`, `list_selected_item_text`, BeOS's
   `B_LIST_SELECTED_*`), under or over the thing, never a thick coloured
   outline; `Palette::selection_edge()` is the hairline for a selected
   thing that stands on its own, so its shape stays crisp. Pick by what
   is selected: a list row or an icon's name sits on the fill; a card is
   `style::card(…, selected)`; a picture, swatch or preview stands on
   `style::selection_plate(window, r, palette)` (a rounded ground
   `metrics::SELECTION_PAD` beyond it, painted first, as Tracker sets a
   selected icon on its highlight); a part of something flush with its
   neighbours (a bar's segment, a map's partition) takes
   `style::selection_wash(window, r, palette)` (the fill, see-through,
   with its hairline inside); something round (a colour swatch) a ring
   of the fill with an edge stroke; a drag's band is
   `style::selection_band`. A group that is current as a whole
   (Deskbar's workspace showing) is lit: `style::lit_ground(palette,
   under)` behind all of it, `style::lit_edges` above and below, its rows
   shading that ground as they would `under`; a row chosen inside it
   takes the menu's selection (`menu_selected_background`), which stands
   out on it where the list's would not. `keyboard_navigation` is only
   where the keys are (focus), never what is selected. app_server's own
   drawing (Twitcher's chosen app, on the same plate) uses the same
   colours.
5. **Lines are one logical pixel** (`style::hline`, `vline`, `frame_rect`,
   `bevel`, `sunken`, `raised`), made a whole number of device pixels:
   `style::line_pixels(scale)` (1 at 1× and 1.5×, 2 at 2×), so at every
   scale all lines are one weight and crisp. A line ending at an edge is
   `hline_above(…, bottom, …)` / `vline_left_of(…, right, …)`, not
   `hline(…, bottom - 1.0, …)` (a pixel off at 1.5×); a line's width is
   `style::hairline(window)`, not 1.0 (it blurs at 1.5×); what fills
   inside a frame of n lines is `style::inset(window, r, n)`, not the
   rect shrunk by n (a crack at 1.5×). Snap rects to the pixel grid
   (`style::snap`; something of a set size centred on a point,
   `style::snap_centred`, which keeps its size; a line that icons and
   text are centred on, `style::snap_coord`, else at 1.25× an icon
   snapped from it and text centred on it round a pixel apart), and lay
   out things that tile (panes, a bar's segments) in device pixels, so
   no seam opens between them. No blurry half-pixel lines. Heights and
   widths are on the 2-point grid (even numbers of points: `metrics`,
   `menu`, a button's width), so they are whole device pixels at 1×, 1.5×
   and 2× and nothing is left a pixel over. Two sizes of text side by
   side on one line share a baseline: `style::baseline_at` gives it,
   `style::label_on_baseline` puts each on it (each centred on its own
   lands a pixel apart at 1.5×). A row whose height is worked out (from
   its text) is rounded up to the 2-point grid, so a list of them stays
   on whole device pixels. See "Fractional scaling". An outline's fold
   mark is `style::latch(window, r, open, color)` (BOutlineListView's
   triangle: right when folded, down when open).
6. **Icons**: `icons::paint(window, bounds, "name")` with names from
   `data/icons/manifest.txt` (or freedesktop names / absolute paths).
   Sizes 16 (lists, Deskbar), 32 (Tracker list), 48–64 (Tracker icon view),
   128 (About boxes). An icon is drawn at its texture's own size, a
   texture pixel to a device pixel, centred in the rect; a size Olo has no
   bitmap for (20 at 1.25×) is averaged down from the next one up when it
   loads. Icons and the wordmark are drawn in the theme chosen in
   Preferences ▸ Themes, and redrawn when it changes; never keep an
   icon's texture yourself. Olo's wordmark is
   `brand::wordmark_centred(window, bounds, dark)` (from `data/brand` or
   the theme's `brand/`, shrunk crisply to the height asked for);
   Deskbar's tile shows it alone, centred (beside the `olo` mark it read
   as "o olo"). Picture files' thumbnails: `pictures::thumbnail(cx, path,
   px)` decodes off-thread and returns `None` until ready (every window
   draws again when it is), never upscaled or blurred. Need a new icon?
   Follow **[ICONS.md](ICONS.md)**: the style guide, subject template and
   required generation workflow (sunburst at max quality, against the
   reference set, candidates reviewed on a contact sheet).
7. BeOS idioms to keep: yellow tab decorator (app_server's job), menu bar in
   every document window, command-key shortcuts (Ctrl on the wire: see
   "Keyboard profiles": app_server's `keymap` turns the profile's command
   key, ⌘ on a Mac or Alt on BeOS, into Ctrl for clients, Caps Lock too
   if chosen, and keeps Twitcher's keys and the desktop shortcuts for
   itself; terminals listed in
   `KeyboardSettings::terminal_apps` get the command key as Ctrl+Shift so physical Ctrl
   reaches the tty; with a Mac's keys, programs that aren't Olo's get ⌥ arrows as Ctrl
   (words), ⌘[ / ⌘] as Alt+← / Alt+→ (Back / Forward, in Chrome and
   Slack alike) and ⌘ arrows and ⌘⌫ as the line's and document's keys
   (Olo's apps get those as they are and the kit makes them so in text
   fields); `KeyboardSettings::num_lock` turns Num Lock on at start
   and when changed, and `apply_input_settings` keeps the Num Lock key's
   state across keymap changes, re-advertising modifiers to the focused
   client), keyboard focus shown as a navigation-blue underline/frame,
   Tracker-style spatial behaviour. Modernise freely where it helps
   (smooth scrolling, hi-dpi, search fields, sensible defaults).
8. **Everything fits** at the window's size, light and dark, at 1×,
   1.5× and 2×, with the longest things it may show: nothing runs past the area
   it was given or over a neighbour, and no fixed text is cut short.
   Check it with `tsession review` at each scale (see "Layout review"
   under Testing) and by looking, with the longest names the view may
   be given.

## The GPUI kit (crates/ui, `olo-ui`)

Every Olo app is built on olo-ui, on Olo's GPUI (gpui/README.md); the
egui kit it replaced left the workspace on 2026-11-02. An app calls
`olo_ui::run(id, build)` and opens windows with `open_window(cx,
WindowOptions, |window, cx| view)`: its GPUI view inside a `Frame`,
which paints the panel behind it, sets Olo's font (Noto Sans, 12 points)
and the palette's text colour, and starts each frame for agents. It ends
when its last window closes. `proxy(cx)` gives any thread a `Proxy` to
send messages the app takes in `message_handler` (a forwarded
`instance::Launch`).

**An entity isn't read while it's updated.** `Entity<T>::update` takes T
out of the app while its closure runs, so reading or updating that
entity again from inside, however many calls deep (a helper that reads
the settings, a view built for a new window), panics: "cannot read T
while it is already being updated". The compiler can't see it (entities
are shared handles, like `Rc<RefCell<T>>`), so: read what you need
before the update, or `cx.defer` what must read it after. Debug builds
name where the conflicting update began ("its update began at
apps/x/src/main.rs:88:9"), and `tools/dev/lockcheck reentry` finds it
without running anything: a rustc driver (tools/reentry) that follows
every update's closure through what it calls and lists each read or
update of an entity of the same type (`// reentry: ok` on the line when
it's another entity of that type). Run it after changing how entities
are used. A window opened inside an update draws its first frame once
the update is over (GPUI defers it; before, it drew there and then and
its views' renders read what was being updated), still before any
event reaches it.

- `theme`: the palette as a GPUI global (`theme::palette(cx)`),
  following app_server's settings from a thread that sleeps until an
  event comes; also the person's choice of how text is drawn on each
  display (Preferences ▸ Display), handed to GPUI.
- `style`: BeOS's drawing primitives on whole device pixels at any
  scale, painted from an element's paint (`canvas`): `fill_rect`,
  `hline`/`vline` (and `_above`/`_left_of`), `frame_rect`, `bevel`,
  `sunken`, `raised`, `vgradient`, `inset`, `snap`, `hairline`, and
  `label` (one line, capitals centred on a y, baseline on a pixel).
  Colours: `c(rgba)`, `tint`, `shade`.
- `agent`: widgets say what they are as they paint (`agent::record(
  window, cx, Node::button(name, bounds))`), so each frame leaves a
  tree the socket (`olo_kit_base::commands`) answers `{"tree":true}`
  from at once; `{"frames":true}` from GPUI's `Window::take_frame_stats`;
  `set_menus` gives the palette and agents the app's commands;
  `set_path(window, cx, path)` says which file or folder a window shows
  (app_server's `{"path":ID}` for its tab's Open in Tracker).
- `widgets::TextField`: one line, input methods, selection by keyboard
  and pointer, words, clipboard, undo; the caret a line wide on device
  pixels, blinking only while it has the keyboard in an active window,
  and for ten seconds after the last keystroke or click (then it stays:
  a window left with a field that has the keyboard draws nothing, as
  GTK's `gtk-cursor-blink-timeout`). Text wider than the field
  scrolls sideways while it's edited, keeping the caret in view
  (`scroll_x`, as BeOS's text views). Unfocused, the field shows its
  start, and the whole text is in a tip on hover (`cut::note`; never a
  password's).
  `TextFieldEvent::Changed` and `Typed`. No frame: callers draw the
  well (`TextControl` does). `set_colors` colours parts of its text from
  the text (a search's filters), `selection`/`set_selection`,
  `select_all_text`, `set_hint` and `focus` for its owner. A right
  press gives Cut, Copy, Paste and Select All. `multiline(enter_sends)`
  makes it several lines wrapped to its width (up and down keep their
  column; Home and End are the line's; ⌘Home and ⌘End the text's;
  ⇧Return and ⌥Return a new line; Return too unless `enter_sends`,
  when it emits `Entered`), scrolled to keep the caret in view, each
  line's baseline on a device pixel; its owner sizes it from
  `rows_at(window, width)` and `TextField::line_height_for` (Telegram's
  composer). ⌘↑ and ⌘↓ are left to the window's menu.
- `widgets::find_bar` (with `find_said`): Find's bar over what it
  searches, a band as a tool bar is, the field's text under the menu
  bar's first title, how many it found ("3 of 90", "Not found",
  "Looking… (12 of 38 pages)"), Previous, Next and Close; Escape
  closes. The view keeps the field and its matches, marks each (the
  yellow hue, see-through) and selects the one shown. Viewer's PDFs,
  texts and formatted documents use it.
- A text selection's colour: `style::selection_behind(p, focused)`
  under the text (text fields, the editor, the text previews),
  `style::selection_over(p, active)` over a picture of it (a PDF page).
- `widgets::TextView` (`text_view(cx, id, |v| v.mono(true).empty_text(…))`):
  a text to read, not editable: wrapped to its
  scroll view, plain or monospaced (`style::mono_font`), coloured by runs
  its owner gives with the text (`set_text(text, Some(runs))`, made for
  `font()`), the note in the middle while empty; selected with the
  pointer (double click a word, triple all), ⌘A, ⌘C and a context menu
  copy. Agents read it as a `Document` with its text. Built on GPUI's
  `StyledText` (wrapping, measuring, run backgrounds); the selection is a
  run background too, as the marks are.
- `style::label_runs`/`label_runs_fit` (one line in runs, cut short to
  fit), `Cell::text_runs` (a list cell's), `ListModel::at_end` (its last
  row shows: a list following what's added keeps following only then),
  `Control::agent_value` (a role, name and value: a filter's menu) and
  `Control::pointing` (the pointing hand: a link).
- The BeOS controls (see them all in `cargo run -p olo-ui --example
  gallery`): `Button` (`default`, `pushed`, `small`, `on_click`),
  `CheckBox` (`mixed`, `on_toggle`), `RadioButton` (`on_select`),
  `MenuField` (label, divider, `quiet_label`: a field set among facts,
  its label theirs and its choice on their values' line; `name`: its
  name for agents and the settings index with no label shown, a field in
  a grid named by its row and column; its menu opens with the current
  choice over the field: `menu::popup_over`),
  `TextControl` (label, divider,
  the sunken well around a `TextField`), `StringView` (`dim`, `bold`),
  `BBox` (etched frame, bold label in its top line, `content_width`,
  `action` for a `LinkButton` at the top line's end; agents read it as
  a `Group` named by its label, round what's in it), `LinkButton` (only
  its words, small, in the link blue, underlined under the pointer,
  `LABEL_ROW` tall: an action in a line with no room for a button),
  `EmptyState` (an icon, a line, a detail and an action button,
  centred in what it's given: a list or pane with nothing in it).
  Each is a `widgets::Control` underneath: an element that keeps
  whether the pointer is over it or holding it down, draws itself from
  that through a closure, takes a click (press and release both on
  it), says what it is to agents, and is drawn faded (half opacity)
  when it can't be used.
  Keyboard navigation, as BeOS's: Tab and ⇧Tab (bound in the Frame)
  move the keyboard through the window's tab stops in drawing order:
  text fields, lists and sliders (their focus handles are tab stops; a
  disabled field drops out), and every button, check box, radio button
  and menu field (`Control::focusable`: a focus handle kept by the
  control's id, `Window::insert_tab_stop`, the label underlined in the
  navigation colour while it has the keyboard, Space clicks it). A
  default button (`Button::default`, `Control::default_key`) answers
  Return from anywhere in its window when what has the keyboard
  doesn't take Return itself (a field's Return is its own, an alert's
  is its action). A canvas
  absolutely placed in a padded element needs `.inset_0()`, not
  `.size_full()`: without insets it starts after the padding.
- `widgets::ToolBar` (`toolbar.rs`): the window's menu commands as
  icons, along its top or down its side (`vertical`), as Gobe
  Productive's tool and format bars and Haiku's BToolBar. Built
  `ToolBar::new(id, name, on_command)`, then its tools: `command(cmd,
  icon)`, `toggle(cmd, icon)` (down while its menu item is checked),
  `picker(name, value, width, make_menu)` and `colors(name, value,
  swatch, make_menu)` (pop-ups showing what's chosen, their menu made as
  they open, `enabled` for the last one), `separator()`, `space()`; then
  `menus(&menus)`, the menus the window's bar shows. Every tool is one
  of those commands: its tip ("Bold (⌘B)"), what agents read (a Button,
  a ToggleButton with whether it's on, a ComboBox with its value, in a
  Toolbar named `name`), whether it can be used and whether it's on all
  come from its menu item, so bar and menus never disagree, and the
  keyboard reaches each through the menus and the palette (tools aren't
  tab stops: Tab is the document's). A command no menu has is a debug
  assertion. Flat until the pointer is over a tool (raised), sunken
  while held or on; icons 16 points (ICONS.md: they must read on the
  light, dark and night panels: the format letters are light metal for
  that). `TIGHT` round and between its rows, so one row is
  `TOOLBAR_HEIGHT`; tools in a group 2 points apart (the review's own
  grid, as a tray's icons). Too narrow, it wraps at separators up to
  `rows(n)` and puts what still doesn't fit in a » menu at its end (as
  the menus have them, a picker as a submenu, groups apart): nothing is
  cut off. `extent(window, length)` is its height (width down a side) on
  whole device pixels, for laying out beside it. `cached(&view, window,
  cx)` draws it in a `ToolBarView` of its own (`AnyView::cached`),
  drawn again only when what it shows changes (a hash of its tools'
  states, picker values and length), its last frame replayed otherwise:
  typing in StyledEdit doesn't lay its forty tools out again (they cost
  ~280 µs a key drawn each frame, ~55 µs cached). It's told in its
  parent's render, so it's marked with `Window::redraw_view` (Olo's
  GPUI: a cached view drawn again in the frame being drawn, which a
  `notify` then would leave for the next). `Menu::swatch(color)` gives a
  menu item a colour swatch in the icons' column (`color::paint_swatch`,
  `ColorWell`'s look).
- Custom drawing in a `canvas` paints with GPUI: `window.paint_quad`
  (rectangles, rounded, bordered; circles as fully rounded quads),
  `style::label` (text), `icons::paint`, `PathBuilder` (filled or stroked
  shapes, small ones with exact coverage), `window.paint_segment`
  (lines of any angle on the GPU, for graphs and strokes:
  `examples/lines.rs`; `window.paint_segments(pieces, width, round,
  color)` for many of one colour at once, a polyline's pieces: one
  operation for the lot), `window.paint_area(from, to, bottom, color)`
  (what's under a segment down to `bottom`, on the GPU in one instance:
  its slanted top antialiased by distance, its sides taking the pixels
  whose middles they hold, so a run of them side by side fills under a
  line with no seam: a graph's fill, where a path would be tessellated
  each frame and filled multisampled through textures as big as the
  window, 45 MB of them for Activity Monitor's) and `window.paint_sector` (a ring's slice, a
  pie's, a disc or a circle's outline on the GPU, analytically
  antialiased: charts, dials, round marks). A picture bent, folded or
  magnified is `window.paint_image_triangles(image, triangles, tint)`:
  triangles of the picture, each corner with where it is on screen and
  where it is in the picture (0–1), on the GPU from the sprite atlas
  (the screen saver's Flip and Lens). Many lines or slices go in
  a `window.paint_layer(bounds, ..)`: one place in the scene's order for
  them all rather than a bounds-tree insert each (a layer draws its
  kinds in a fixed order, fills before lines before sprites, so a mask
  that must go under lines needs a layer of its own before them).
- Alerts: `WindowOptions::alert(title, width, height)` (fixed size,
  xdg-dialog modal over the app's window with the keyboard, floated);
  `widgets::AlertFrame::new(icon)` (the stripe with the icon over it,
  children in the column beside it), `alert_title`, `paragraph(text)`
  (wrapped, `style::LINE_HEIGHT`; `paragraph_height` sizes the
  window before it opens), `button_row()` (right-aligned, default
  last), and `Conversation` with `ConversationField` for PAM's prompts
  (polkit, sudo). The alert's root takes `alert::CONTEXT` and handles
  `alert::Cancel` (Escape) and `alert::Default` (Return); a text field
  emits `TextFieldEvent::Entered` for Return first.
  `TextField::password(true)` shows bullets and keeps no copies (no
  undo, no copy); `clear_secret` zeroes it; `set_enabled(false)` dims
  it and takes no typing.
- `widgets::open_alert(cx, title, icon, message, buttons, on_choose)`:
  a whole BAlert (sized from its message and buttons before it opens;
  Return the last, Escape the first of several; `on_choose(None)` when
  closed from its tab).
  `widgets::open_timed_alert(cx, title, icon, message, buttons,
  Timeout { at, choice, says }, on_choose)`: one that answers itself
  with `choice` at `at` (an `Rc<Cell<Instant>>`: set it later for more
  time), the seconds left counted down in a line under the message
  (`says(left)`: "Reverting in 12 seconds."), woken once a second while
  it's open and not between: for a change that may have left the
  person unable to answer (a display gone dark), undone unless kept.
  Preferences ▸ Display asks so after a new resolution and HDR on.
- `widgets::ScrollView::new(id, &handle)` (a `ScrollHandle` kept in the
  view's state): what it holds scrolled in a sunken frame with BeOS
  scroll bars (`horizontal`, `vertical`, `border`, `focused`,
  `background`); `ScrollBar::vertical(id, handle)` drives any
  `ScrollHandle` (a `uniform_list`'s too): arrows that scroll on while
  held, a track click pages, the knob drags; agents read it as a
  `ScrollBar` with its position, 0 to 1. In a frame it places itself
  inside the frame's two lines on device pixels (`in_frame`).
  `auto_hide(true)` shows the vertical bar only while what it holds
  overflows (panes side by side); its content scrolls (clipped to the
  view, measured) with the bar hidden too, so the bar comes as soon as
  it overflows (not scrolling, a hidden bar's content ran past the view
  over what's below and never showed the bar).
- `widgets::SlimScrollBar::new(id, top, shown, total).on_scroll(|to,
  window, cx|)`: the slim one, there when wanted, for content a BeOS
  bar's column would cost (Terminal's scrollback; use the BeOS bar
  elsewhere). Laid over the box it's given: a thin rounded thumb at its
  right edge (`THUMB` 6, `THUMB_WIDE` 8 under the pointer, with a faint
  track), no arrows. It shows when the pointer comes within `SLIM_NEAR`
  (28) of the edge or the view scrolls (moved against both ends: not
  output added at the end it keeps to, nor a resize), stays while the
  pointer is near or the thumb held, and fades `LINGER` (0.9 s) after,
  over `FADE` (0.22 s): one timer, then the fade's frames, nothing while
  it rests. Hidden it takes no clicks. The thumb drags the view with
  the pointer, a click in the track pages (0.9 of what shows) towards
  it; `on_scroll` hears the top asked for, in the units it was given
  (lines, points). Agents read a `ScrollBar`, its value 0 to 1.
- `style::pill`/`pill_on_baseline`/`pill_width`/`pill_height` (a count
  or tag in a pill, its text on a label's baseline beside it),
  `baseline_at` and `label_on_baseline` (things of different sizes on
  one baseline), `card_ink`/`card_paper` (cards on a gutter).
- `widgets::StatusBar::new(fraction).label(..).trailing(..)` and
  `level_bar` (the bar alone).
- `copy::secret(cx, text, clear_after)`: a password or one-time code on
  the clipboard, marked for clipboard managers to leave alone
  (`x-kde-passwordManagerHint`), and taken off after `clear_after` if
  it's still ours (`App::withdraw_from_clipboard(&item)`, Olo's GPUI:
  nothing if something else was copied since). An app that quits
  while what's on the clipboard is its own runs on with its windows
  gone (`App::quit` closes them) until something else is copied or it
  takes its own off, so what it copied can still be pasted; launched
  meanwhile, it opens a window again and
  carries on.
- Pinches: GPUI's `PinchEvent` (Olo's: the Wayland client binds
  `zwp_pointer_gestures_v1` and turns its pinches into it: where, the
  scale since it began, `step` since the last event, the phase), and
  `on_pinch` on any element as `on_scroll_wheel`. A touchpad's pinch
  (with Preferences ▸ Touchpad's "Pinch to zoom") and a touch screen's
  two fingers on an Olo app both come so. `ImageView` (Viewer's
  pictures, Quick Look) and Viewer's PDF view zoom by `step` about the
  fingers.
- `widgets::ColumnListView::new(id, &model, columns, rows, cell)`:
  BColumnListView, its state in a `ListModel` entity the app keeps
  (`ListModel::new(cx, multiple)`; `state` is the shared
  `olo_kit_base::list::ListState`: its selection,
  anchor, cursor and sort). Only the rows in view are built
  (`uniform_list`); `cell(&mut Cell, window, cx)` draws each
  (`cell.text(..)`/`text_with(..)`, cut short with "…"). A header
  click sorts (`on_sort`), the dividers drag; a title dragged sideways
  moves its column (past a neighbour once the pointer passes its
  middle, so the others make room as it goes, as BColumnListView's
  did), and the cells, the sort and `cell.column` keep each column's own
  index wherever it shows. The order and widths are kept for the app by
  the list's `id` and the columns' titles (`state/olo/tables/<app>.json`,
  written with the windows' sizes: a column added since goes where it
  was given), so the person's arrangement comes back; an agent reads
  the `ColumnHeader`s left to right and moves one by dragging it, an app
  with `ListModel::column_order`/`set_column_order`. A click, ⇧ and ⌘ clicks,
  a band, the arrows, Page Up and Down, Home and End (⇧ grows), ⌘A,
  and typing a name (`type_select`) select (`on_select`); a press on a
  selected row keeps the selection until the release (for dragging);
  double click or Return opens (`on_open`); a right click keeps or
  makes the selection and calls `on_context` with where; `on_press`
  sees a left press first, with the row and where, and takes it by
  answering true (a tree's disclosure triangle, drawn by the cell: the
  press opens or closes the row and leaves the selection); a press on a
  selected row, or on a name, that moves three points calls `on_drag`
  with the row (for the app to carry it: DiskUsage's Collector). Columns
  wider together than the view scroll sideways under a horizontal bar.
  `ListModel::reveal(row)` scrolls a row selected in code into the
  middle; `typing()` says a name is being typed (a space is part of
  it). Agents read `Row`s (named by their cells' text, selected or
  not) and `ColumnHeader`s (one scrolled out of view sideways is none
  wide at the view's edge, not clickable). `row_key(|row, cx| u64)`:
  a number that changes when what a row shows does (its line's id and
  what a search marks in it): such a row whose key and look are as last
  frame's is replayed, moved if scrolled, rather than painted again
  (`Window::paint_memo`; Log Viewer's screen of 22,000 glyphs). Give it
  for any list of many rows. `no_header()` shows the rows
  alone (a list of things, not of columns: Passwords' items), its
  frame's line along their top; a cell drawn by hand (an icon and two
  lines) says its text to agents with `cell.say(text)`.
  `examples/list.rs` shows one.
- `style::label_fit` (a label cut short with "…" to a width, measured
  as shaped), `style::selection_band`, `style::selection_wash` (a thing
  selected in place: the selection colour see-through, its edge
  inside) and `style::snap_centred`.
- `style::key_combo(window, cx, right_center, keys, color)` and
  `key_combo_width`: keys as menus write them ("⇧⌘P"), each symbol
  laid out on its own a point closer to what follows; one about as tall
  as the capitals (⇧ ⌘ ↵ ⌫ ⏏) is set on their line by its outline
  (Noto's ⇧ sits a pixel low; kept per symbol). `style::DETAIL_SIZE`
  (11 points) is for secondary words (a row's subtitle and kind, a
  chip, a caption, a small button); `SMALL_SIZE` (10.5) for graphs'
  labels and headings in capitals. `style::soft_shadow` (an overlay
  panel's), `icons::paint_faded` (an icon of something unusable).
- `widgets::Graph::new(id, series, now)` (live series of timed samples,
  time running right to left, `now` at the right edge: lines drawn as GPU segments over faint
  fills, a grid in quarters and time lines that move with the samples,
  a hover readout, `on_click`/`on_hover` with the time, the next frame
  when the samples have moved a device pixel when `smooth`; agents read
  each series' latest value), `widgets::CellGrid` (Pulse's LED wells,
  `GridCell`s lit in colours, `on_click` with the cell; a click either
  takes is its own, not the clickable card's it's on), and
  `charts::paint_sparkline`/`paint_bars`/`paint_meter`/`paint_well`
  (small graphs painted in a canvas: Deskbar's widgets),
  `widgets::legend`/`legend_height` (swatches, names and values in
  wrapping rows, each name and value on one baseline; laid out by their
  lengths, the review keeps them on a grid of their own, as it does a
  `TabView`'s tabs). A window's graphs move together on one clock,
  ticking at the finest step of those drawn (graphs of many sizes each
  on its own timer drew a frame for each: twice the frames); each
  graph's grid, fills (`paint_area`) and lines are one layer.
  `widgets::Spectrum::new(series, low, high)` is a `Graph`'s look for
  sound: each `SpectrumSeries` (a name, a colour, a level in dB a band,
  bands spaced evenly in log frequency from `low` to `high` Hz, a fill
  if asked) as a line, `floor(db)` at the bottom and 0 dB at the top,
  lines at 100 Hz, 1 kHz and 10 kHz named under the well
  (`SPECTRUM_LABELS` of its height) and every 20 dB; a silent series
  isn't drawn; agents read each series' loudest band. The levels are
  the app's (`olo_voice::spectrum` makes them from samples): it draws
  what it's given, so a live one is drawn each frame by a small view of
  its own, the rest of the window cached (Try Voice Processing).
  `GraphLayer::new(shelf, now)` draws graphs over a view that stays
  still: the view puts each with `graph.on_layer(&shelf)` (it keeps the
  graph's place and says where it is, and its clip, as it's laid out)
  and empties the `GraphShelf` as it renders; with both cached, a graph
  moving a pixel redraws the layer, not the cards, legends and figures
  around it (Activity Monitor's CPU section: 4.2 ms a frame to 1.4).
- `StringView::wrap(true)` (a paragraph, as tall as it wraps to),
  `StringView::size(points)` (a title's or lead's size, its row grown to
  fit), `StringView::beside(points)` (on the baseline of the text of
  that size beside it in its row: a small one centred by itself sits a
  point higher than the plain text beside it; a table's small column
  names on its heading's line, Preferences ▸ Notifications; drawn
  directly, `style::baseline_at` with `style::label_on_baseline`, as
  Backup's list rows' sizes beside their check boxes), `widgets::KeyHint::new("Space", "Next")` (a key's cap and what
  it does, `enabled(false)` dim: for views driven from the keyboard,
  where nothing needs the pointer; agents read "Space: Next"; the cap's
  text and what it does are drawn on one baseline, so they stay level at
  every scale), `widgets::Banner::new(Tone::Success, title, line)`
  (news to be seen at once, not read in a status line: a box across its
  column tinted green or red, a large round check or cross
  (`widgets::verdict_mark`), a bold title, a line, `.trailing(…)` what to
  do next; `banner_height()` tall, and `banner_back(window, b, None, p)`
  draws a quiet box of the same shape for what stands in its place
  until it comes, so nothing moves when it does: the palm trainer's
  finished step and its summary),
  `widgets::heading(text)` (bold, a label row tall) and
  `widgets::separator()` (a groove across).
- `widgets::TabStrip::new(id, tabs, active)`: a row of document tabs
  under a menu bar (Terminal's shells), `STRIP_HEIGHT` (32) tall, each
  a `StripTab { label, icon, status, badge, group, faded }`. Everything sits
  inside the band, a line under it: the tabs are their icons and titles
  on the band, the selected one on a raised plate (`PLATE`, 24, rounded
  7, centred) lit from above as BeOS's controls are (a touch lighter at
  its top, its rim's top edge lit and tapering into the corners, a soft
  shadow under it); one under the pointer on a faint plate, its title
  in full ink, its close box shown; a faint line between the others.
  Each tab is as wide as its title needs (between `TAB_MIN` 140 and
  `TAB_MAX` 320, or `max_tab_width`), the widest cut back alike when
  they don't all fit (`fit_widths`, on the 2-point grid, up where they
  fit so no title loses a fraction of a point), titles cut short with
  "…". `.groups(vec![StripGroup { name, color }])` and each tab's
  `group`: a group is a capsule of its colour (22% in the dark, 16% in
  the light) round its chip (its name, bold, on a pill of the colour,
  `chip`/`chip_width`/`CHIP_HEIGHT`, which Terminal's pane heads use
  too) and its tabs, rounded `TAB_RADIUS + HALO` so its corners are
  concentric with the plates 2 points inside it; no lines. With
  `.group_selection(chosen)` a chip filled is a group chosen (or shown:
  Terminal fills every chip while every group is tiled) and an outlined
  one not; a click gives `ToggleGroup(g)`, the right button
  `Group(g, pos)`. A tab with `faded: Some(why)` is left out of
  something the window shows (Terminal's Hide When Tiled): its icon and
  title drawn at `STRIP_FADED` (0.45), its plate, close box and badge as
  any tab's, and agents read `why` in its name ("…, hidden when
  tiled").
  `.side()` puts it down the content's left: rows, the selected one
  raised the same way, a group's chip heading its rows in a capsule,
  New Tab a row after the last, the right edge dragged to resize
  (`Resize`). Its icons and its chips' names are at `WINDOW_INSET`,
  under the menu bar's first title (`SIDE_FACE`, `side_chip`), and its
  first row is centred on `STRIP_SIDE_HEAD` (32, as a strip along the top: the pane heads beside
  it), so a chip heading it is where it is along the top.
  Agents read a side chip and New Tab as what shows (the pill; the +
  and its name), though their whole rows take a click. A tab dragged rides on a raised plate under the pointer,
  its slot left empty, a mark where it would land; out of the strip
  (`TEAR`) it's torn off (`TearOff`, `TearOffGroup` for a chip). A
  window of the app moved over the strip marks where its tabs would go
  (`strip::window_over`, from app_server's `WindowOver`), and let go
  there `strip::on_window_dropped(cx, handler)` gets the place
  (`strip_place`). The pointer coming to a tab or leaving it redraws
  (a move listener comparing what's under it with what was painted).
  What the person did comes to `.on_action(|StripAction, window, cx|)`.
  Agents read each tab as a `Tab` (its title, status and group in its
  name, `selected`), its close box, the chips and New Tab as buttons.
- `WindowOptions::max_size(w, h)` (GPUI's `window_max_size`, xdg
  `set_max_size`): a window no larger along a side than its smallest is
  fixed along it, and app_server neither tiles nor groups it (Terminal's
  Find panel, fixed in height). `WindowOptions::under_pointer` and
  `olo_ui::begin_move(window)`: a window torn off a strip opens with its
  tab under the pointer and moves with it.
- `MenuBar::new(..).raw()`: the bar's shortcuts as the app presses them
  (a terminal's ⌘ keys), not the profile's; `menu::resolve(&mut menus,
  raw)` gives a context menu the same keys (`Menu::keys_from(&bar)`
  after resolving the bar). `keys::unshifted(key)` maps the symbol GPUI
  names for a shifted key ("_") to the key and Shift ("-", true);
  `gpui::right_control_held()` says whether the right Ctrl (the Mac
  profile's ⌘) is down, for a raw-keys window telling ⌘ from Ctrl.
- `widgets::Slider::new(id, value, range)`: BSlider (label and value
  above, the bar filled to a raised thumb, `limits` under the ends;
  `step`, `fill`, `width`); a press or a drag sets it, the arrows, Home
  and End once it has the keyboard; `on_change` gets each value.
- `file_panel::show(cx, title, FilePanel::open(dir) / save(dir, name) /
  folder(dir), on_done)`: the shared Open, Save and folder panels (see
  "What every app does the same way"), in a dialog window of their own; `on_done` gets an
  `Answer` (the `PanelResult`, the filter and choices made, the folder)
  once, Cancelled if the window is closed. `types`, `filters` (a Show
  pop-up), `multiple`, and the app's `choices` (check boxes or
  pop-ups). Listing on a worker, following the folder as it changes;
  the folder pop-up goes up, to the standard folders, favourites and
  recent ones; Save asks before replacing; Space looks in the Viewer;
  an item's menu has Quick Look, Open With ▸ and Show in Tracker.
- `open_with`: Open With, the same in every app that shows files
  (Tracker's windows and desktop, Quick Look, the file panels).
  `open_with::kind_of(names)` is the kind a selection all is (None:
  mixed); `open_with::menu(kind)` the submenu: the apps worked out for
  it (`olo_xdg::open::offer_cached`, so warm the kinds where the files
  are listed, off the UI thread: `olo_xdg::open::warm`), the one it
  opens in marked "(default)", Other…, and Always Open With ▸ (the
  kind's checked) to make another its default; a kind no app knows gets
  Other… alone. `open_with::is_command(cmd)` tells its commands, and
  `open_with::run(cmd, kind, paths, cx)` does one: opens with the app
  (Always: and makes it the kind's, `open::set_default`), on a thread,
  or opens Other…, `OpenWithWin` (every app that opens files,
  `open::every_app`, read on a worker, found by typing part of its
  name; ↑↓ and Return; "Always open files like … with it"), a dialog
  over the window in front that closes itself
  (`open_with::window_options()` to open it yourself).
  `examples/panel.rs` opens each.
- Menu items take icons (`Menu::icon(name)`, an item's or a submenu's):
  a column of them between the check marks and the labels, faded when
  the item can't be chosen.
- `Button::on_click_at(|bounds, window, cx| ..)`: told where the button
  is, to open a menu under it (`menu::popup(.., bounds,
  Corner::BottomLeft, ..)`).
- `olo_ui::on_upgrade_due(cx, f)` and `olo_ui::upgrade_waits_while(cx,
  busy)`: a server finishes what is under way before it becomes its new
  version (Token Pool's drain); `busy` is asked four times a second
  while an upgrade is due.
- `widgets::tooltip(text)` for an element's `.tooltip(..)` (the
  palette's tooltip colours, BeOS's pale yellow in light mode).
- `olo_ui::stay_resident(cx)`: the app keeps running with no windows
  (an agent); otherwise it quits when its last window closes (GPUI's
  own quit-on-last-window is gone: olo-ui decides).
- `icons::paint(window, bounds, name)`: Olo's icon set (or a theme's,
  or a file's), from the bitmap made for the device size it's shown at,
  a texture pixel to a device pixel; decoded once, on a worker, the
  windows drawn again as each arrives.
- `menu`: BeOS menus (`Menu::new(title).item(label, command).checked(
  on).separator().submenu(m)`), shown as Wayland popups (GPUI's
  `WindowKind::Popup`, an xdg_popup over the window at a corner of an
  anchor rectangle, flipped or slid by app_server to stay on screen):
  `menu::context_menu(window, cx, menu, position, on_choose)` and
  `menu::popup(window, cx, menu, anchor, corner, on_choose)`. Drawn as
  BeOS drew them; ↑ ↓ → ← Enter Space Escape; a submenu opens
  beside its item; items are `MenuItem`s to agents. `Menu::entries`
  flattens one for the palette (`agent::set_menus`). `Menu::sorted()`
  puts a list's items in alphabetical order by what they say (natural
  order, case aside), each run between separators on its own, its
  submenus' too, all the way down (Deskbar's Applications, Other
  Applications and Preferences). Keys are drawn as
  the kit's key combos (`style::key_combo`: each symbol on its own, ⌘
  and ⇧ tightened and lifted onto the capitals' line). `.minimized(on)`
  marks an item a minimized window (a hollow diamond where the check
  goes, a lighter label, `minimized` to agents), as a Mac's Window menu
  does. A menu shows no separator at either end or two together, so one
  built with items it has only sometimes needs no care with them (a
  menu field's, whose items are counted, is left as it is). A press anywhere in the window a menu opened over puts it away
  (the `Frame` sees it first: app_server's popup grab hands an app's
  own window the presses outside its menu), and that press doesn't
  open it again on what opened it, so a second click on a button
  closes its menu. `menu::popup_then(.., on_close)` is `popup` with
  word when it has gone, for what shows its menu open (Deskbar's rows).
  (Still to come: scrolling a menu taller than the screen, and
  press-drag-release choosing from the press that opened it.)
- `MenuBar::new(menus, on_command)` at the top of a window's view: the
  BeOS menu bar. A press on a title opens its
  menu under it; with one open, sliding across the bar opens the
  others; a press on the open title puts it away (one that slid open
  stays for its first press, as on BeOS), and so does a press anywhere
  else in the window (`press_in`). The bar knows its menu by its popup,
  not its place: the popup a slide replaced goes after the new one
  opens, and taken by place the bar forgot the new menu and opened
  another over it, which xdg-shell forbids (a menu may open only on the
  topmost), so app_server ended the app's connection (Olo Doctor,
  2026-11-01). GPUI guards against any app doing that: before a menu
  opens, our menus showing outside its parent's chain are taken down
  (`withdraw_stray_menus`, with a warning). Items take keys with
  `Menu::key("w")` / `shift_key("z")` / `shortcut(..)` (a Mac's: the
  command key is Ctrl to an app), and `menu::resolve` gives every item
  the keys its command has in the keyboard profile (`olo_kit_base::keys`:
  Mac, Windows or BeOS keys and labels, followed live from app_server's
  settings). The window's `Frame` has the keyboard when
  nothing in it does and takes the bar's keys that nothing else took
  (a text field's ⌘C stays its own). With no `agent::set_menus`, the
  command palette reads and invokes the front window's bar.
- `MenuBar::of(&app, |a, cx| a.menus(cx), on_command)`: the same bar,
  its menus built from `app` (the window's view) when it changes
  (`cx.notify()`), not each time the window draws. GPUI draws again
  every view above one that changed, so a window whose picture moves (a
  video, a live agent screen, a scan's progress) drew its whole menu
  bar again each frame: building, resolving and copying the Viewer's
  menus was 32 µs of its 261 µs a 4K60 frame (210 after). Opening a
  menu, a key the bar takes and an agent's or the palette's look build
  them afresh, so what they say is never stale; only the titles wait
  for the app to change. The Viewer, Agent Screens, Notifications'
  history, Disk Usage, Drive Setup, Telegram and Doctor use it; a window
  that notifies itself for everything (Terminal's `Shell`, which
  redraws for output) gains nothing from it.
  `Menu::keys_from(bar)` gives a context menu the bar's keys.
- `WindowOptions::floating()`: the xdg-toplevel-tag `floating`, so
  app_server floats it over auto-tiled windows. `child()`: the window
  belongs to the app's window that has the keyboard (its xdg parent:
  a chat's Info over its chat), opening over it rather than as a tab
  beside it; `dialog()` is that and never a tab.
  `restore_key(key)`: what app_server knows the window by when its app
  comes back (a restart, an upgrade), the xdg-toplevel-tag word
  `restore:KEY`: its place, workspace, tab's place in its group and
  tile are this window's and no other's of its title. A key the same
  for the same window each time, unique among the app's (a document's
  path, a number kept in the app's saved state: Terminal's).
- Cached views (`AnyView::from(entity).cached(style)`): GPUI replays a
  view's last paint when nothing it drew changed (it wasn't notified,
  its bounds are the same). What agents read is kept with the frame
  (`Window::push_paint_record`), so a replayed view still says what it
  shows. A cached view's root is laid out as a root (its offsets
  ignored): put what places its content inside it (Telegram's panes).
  Its size comes from the style it's given, not its content. It's
  drawn again when it's notified, not when a model it reads (a
  `ListModel`) is: observe those (`cx.observe(&model, |_, _, cx|
  cx.notify())`). And GPUI draws again every view above one that
  changed (their renders run; their cached children are replayed): so
  keep what changes often (a caret, a graph moving a pixel) in a small
  view, and what's costly beside it in cached views, not around it.
  `part::Part::new(&app, draw, cx)` is such a view drawn from the app's
  entity (`draw(&mut App, window, cx)`, drawn again as the app changes;
  `.observe(&model, cx)` for a list's model it shows too), placed with
  `part::cached(&part, style)` (its size in the style). Activity
  Monitor's window is parts (menu bar, sidebar, the section's cards,
  the process list) and a `GraphLayer`: a graph's tick redraws the
  graphs and the window's few boxes around them. Passwords' panes are
  parts, and a one-time code counting down notifies the item pane's
  part alone, once a second.
- `Window::warm_glyphs(font, size)` (Olo's GPUI) has a face and size's
  printable characters drawn ahead, off the UI thread, so the frame that
  first shows them only copies them (a glyph drawn the first time costs
  a frame ~12 µs; a chat of new text, 3 ms). GPUI does it itself for
  running text once it has drawn a few dozen of a face's glyphs; ask
  for what a window will show soon that it hasn't yet (Telegram: the
  conversation's faces, at start). It follows the window to another
  scale or text rendering.
- `Window::warm_text(font, size, texts)` (Olo's GPUI) has the colour
  glyphs (emoji) of `texts` drawn ahead the same way, in the order
  given (shaped, then drawn on the warming thread without holding the
  text system, 16 texts at a time), kept for every window of the app
  after, and drawn again for a display of another scale the window goes
  to: the emoji picker's ~1,900 at open, so a search's first frame
  copies them (its slowest typing frame at 2×: 8.1 → 1.0–1.5 ms).
- `Window::activate_window_with_token(token)` (Olo's GPUI) raises a
  window with an activation token another program gave (a notification
  clicked: `NoteEvent::Action`'s), which lets it take the focus.
- `WindowOptions::layer(namespace, layer, (top, bottom, left, right))`
  makes a layer surface (Deskbar, notifications, an overlay) instead of
  a window; `layer_options(|l| ...)` sets its exclusive zone, margins,
  keyboard (`LayerKeyboard::OnDemand` for the desktop, `Exclusive` for a
  modal overlay) and output; `no_panel()` leaves its background to it
  (transparent: what's under it shows). Menus open over it as over a
  window (`examples/layer.rs`); a layer that only carries a menu takes
  the keyboard `OnDemand`, as app_server gives every key to an
  exclusive one, past the menu's grab. `menu::context_menu_then(window,
  cx, menu, at, on_choose, on_close)` is `context_menu` with word when
  it has gone (to take such a layer away; `MenuPopup::close` puts it
  away, for a click on the layer, which the grab lets through as the
  same program's). Open a window from a paint after the frame
  (`window.defer`): opened during it, the new window's first draw
  clears the frame's element arena under the one painting.
- `tip::hover(window, cx, key, text, anchor, hovered)` (from an
  element's `on_hover`) and `tip::pressed(window, cx, key)`: tips in a
  popup of their own (GPUI's `WindowKind::Tip`, an xdg_popup that takes
  no input and grabs nothing), for a window too small to hold them
  (Deskbar's tray): after the pointer rests half a second, at once if
  one showed a moment ago, not after a press; below the anchor, else
  above, slid to stay on screen; in the tooltip colours, wrapping past
  360 points. `widgets::tooltip` is GPUI's own, inside the window, for
  the rest.
- `dnd`: dragging out to another app or a folder (GPUI's
  `Window::start_drag`, Wayland's data source with a
  picture surface): `dnd::files(paths, copy, move)` (text/uri-list and
  the paths as text), `dnd::as_link` (Olo's make-a-link type, for an
  application dragged out of Deskbar), `dnd::picture(window, cx, icon,
  size, label, hotspot)` (the icon faded a little, its name below in
  the selection's colours, on whole device pixels; the name drawn on
  the CPU by `TextSystem::line_mask`), and `dnd::start(window, cx, drag,
  done)` while the press is down (`done` gets what it was dropped as).
  Ask for the icon's bitmap ahead (`icons::bitmap`) as the press comes,
  so it's decoded when the drag starts. Drops come as GPUI's
  `on_drop::<ExternalPaths>` (an app's own drag isn't dropped back on
  it).
- `widgets::ImageView` (`image_view(cx, zoomable)`, `set_picture`): a
  picture (anything `Rgba`: its size and premultiplied pixels, as
  olo-preview's `Bitmap`) fitted to its view at a point a pixel at
  most, or zoomed (`zoom(Some(true))` a step in, `Some(false)` out,
  `None` to fit; `set_scale(1.0)`: Actual Size) and panned by dragging. Shown pictures are resampled exactly to
  their device size (Lanczos, `fast_image_resize`) on a worker, the last
  exact one standing in until the next comes; magnified, it is cut into
  512-pixel tiles drawn pixel for pixel, and a small one (256 pixels or
  less) is drawn with nearest sampling so its pixels stay sharp.
  Transparency shows over a checkerboard. `picture_bounds` says where it
  is drawn (an overlay's place), `set_still` stops pan and zoom,
  `on_drag_out` takes a drag where it doesn't pan; `resampled(src,
  size)` is the resampling alone (Lanczos made smaller, Catmull-Rom made
  bigger: a 1024 × 640 screen magnified to 1421 × 888 in 3 ms against
  9, `resample_timing`).
- A view that fills its box (`size_full`: a `ColumnListView`, a
  `ScrollView`) put in a plain `div` its flex parent stretches comes
  out none wide in GPUI's layout: make that `div` a flex column
  (`flex().flex_col()`, the view's place `flex_1`), as Agent Screens'
  History list sits.
- `style::spinner(window, centre, radius, colour)` (a busy spinner, a
  turning arc as a GPU ring slice; ask for the next frame while it
  shows),
  `widgets::check_box_look` (a check box's look alone, for one drawn in
  a canvas: a task list's), `TextField::font(font)` and `max_len(n)`.
- Olo's GPUI: `TextRun::size` (a run at a size of its own in one line:
  code in a paragraph); `Window::paint_image_nearest` (a picture drawn
  with nearest sampling); `Window::set_keep_awake(on)` (idle-inhibit
  while the window shows); `Window::display` is the display a window
  will open on before it has, when there is only one it may (to size a
  window for its content); displays' sizes are xdg-output's, in points,
  so right at fractional scales (wl_output's whole-number scale made a
  1.5× display 1280 points wide, not 1707), and an output's size and
  refresh are its current mode's (wl_output lists every mode; GPUI took
  the last).
- An overlay over something in the flow (a frame over a picture) is
  `.absolute().inset_0()`: an absolute element without insets sits
  where the flow would put it, after what it was meant to cover.
- `brand::wordmark_centred(window, bounds, dark)`: the wordmark (the
  theme's, light or dark), decoded and trimmed once on a thread,
  averaged down to each height once, a texel to a device pixel;
  `Drawn::Loading` keeps its place, `Missing` asks for the name instead.
- `style::lit_ground`/`lit_edges`: a group current as a whole
  (Deskbar's workspace showing).
- `cx.close_window(handle)` (GPUI): closes a window even from inside an
  update of it (its own button, key or menu command), where
  `handle.update(.., remove_window)` silently does nothing.
- Keyboard hints: app_server asks by surface (`{"hints":[ids]}`); each
  window answers with its `Window::surface_id()` and what can be picked
  in it. A layer surface's tree carries its namespace (`layer`).
- Upgrades: when olo-session signals an upgrade (SIGURG) and the app's
  binary was replaced, or app_server restarts in place, `on_upgrade`'s
  state is saved and the app re-executes (as the new version), finding
  it in `restored_state()`. Olo's GPUI destroys
  every object it makes for a window with it (its fractional scale, its
  xdg-dialog, its idle inhibitor): one left alive names a surface that's
  gone, whose id goes to something else, and olo-wlproxy making it again
  on the new app_server named that (a protocol error that ended
  Notifications on a reload). The proxy now treats an object named in a
  recipe but made after it as gone, for programs that leak them.
  An upgrade restarts olo-wlproxy and app_server one after the other,
  so a program can lose its window's Vulkan surface mid-frame, or the
  compositor again while it reconnects: blade answers a lost surface
  (configuring, acquiring, presenting) with no frame rather than a
  panic, the renderer makes a new surface on the same window
  (`BladeRenderer::remake_surface`, at most every 2 s), and a program
  re-executing connects again if the compositor goes before its
  registry answers. (Telegram panicked on both, 2026-11-02, so an
  upgrade closed it; tried by replacing a copy of its binary, SIGURG
  and `tsession reload` together, twelve times running.)
- `Window::activation_token(done)` (Olo's GPUI): an xdg-activation token
  from the last click in the window, for another program to raise its
  window with (Notifications hands it to the app whose card was
  clicked); `done(None)` when the compositor has none to give.
  `Window::export_handle(done)` (xdg-foreign): the handle another
  program's window can name as its parent (`foreign_parent`) to open
  over this one, exported on first asking and kept until the window
  closes; `done(None)` without xdg-foreign.
  `Window::set_layer_keyboard(LayerKeyboard)` changes a layer surface's
  keyboard as it shows (`OnDemand` while a banner has a reply field,
  `None` otherwise).
- External pictures (Olo's GPUI): another program's dmabuf shown in a
  window by the compositor itself, never copied (Agent Screens' Live).
  - `Window::set_external_buffer(key, Some(ExternalBuffer))` sets the
    picture `key` shows: an id, size, DRM format and modifier, its
    planes' descriptors, and `released`, told the id once the compositor
    is done with it. It's made into a `wl_buffer` once per id (a few
    kept) and shows at once, with no frame drawn.
  - `Window::place_external(key, bounds)` is called as the frame paints.
    It's kept as a paint record, so a cached view's replay brings it
    along. A key not placed in a frame is hidden, and None lets it go
    with its buffers.
  - `Window::external_bounds(bounds)` is where it shows, to the device
    pixel: corners at whole points, then the device pixels the compositor
    rounds them to. Draw round that.
  - `App::external_modifiers(fourcc)` lists the modifiers the compositor
    takes (linux-dmabuf v3's events). It's empty without a subcompositor,
    viewporter or linux-dmabuf.
  - On Wayland it's a desynchronized `wl_subsurface` with an empty input
    region (the pointer reaches the window under it), scaled by a
    `wp_viewport`. When the place changes with a frame, it's synchronized
    until that frame's commit, so window and picture move together.
- Ordinary hitboxes don't block what is under them: a `Button` or
  `TextControl` placed over something clickable (a card) needs
  `.occlude()` on its wrapper, or the click reaches both.
- `style::monogram(window, cx, r, letter, fill)` (an app without an
  icon: its initial on its colour, `ink_on(fill)` black or white by
  contrast; a grid of its own for the review), `style::stop_mark`
  (BAlert's stop sign, small: an error's title), `style::latch` (a
  folding group's triangle), `style::close_box` (a ×, crisp at any
  scale). `CheckBox::height(h)` and `RadioButton::height(h)` give a
  check box or a radio button a row's height, so it centres on the same
  device pixel as a field, a button or a label beside it at fractional
  scales (elements of different heights centred in one row round their
  tops apart: a radio beside a button was a device pixel off at 1.25×).
  A `BBox` tells the review where its inset is on the device pixels
  layout rounds its children to; measured unrounded, anything flush
  with the inset (a full-width note, a button at the right) read as a
  point past it at 1.25× and 1.75×.
- Wrapping (Olo's GPUI): a space after a word hangs past the edge rather
  than pushing that word down a row, as typesetters and CSS wrap
  ("…with Ann at" fits when "at" does); leading spaces still count. The
  multi-line field keeps its caret at the edge among hanging spaces.
- `WindowOptions::foreign_parent(handle)` (xdg-foreign): a window over
  another program's (a portal's dialog over the app asking; `wayland:`
  stripped, nothing if empty). `file_panel::window_options(title)` and
  `show_in(cx, options, panel, on_done)` open a file panel so;
  olo-print's `panel::view::show(cx, options, pages, on_done)` the Print
  panel.
- `widgets::ColorControl::new(id, rgb)` (BColorControl: grey, red, green
  and blue ramps with markers to click or drag, the lane pressed in
  keeping the drag, and Red/Green/Blue fields following the colour unless
  typed in; `on_change` gets each new colour) and `ColorWell::new(color,
  size)`, its sample; `color_control_height()` sizes a well beside it.
  `pictures::thumbnail(cx, path, px)`: a picture file no bigger than `px`
  device pixels, decoded on a thread of its own and kept by path, mtime
  and size; None until it's ready, then every window is drawn again (so
  a view just asks again). `icons::paint_in(window, b, theme, name)` and
  `brand::wordmark_centred_in(window, b, dark, theme)` draw in a theme
  other than the current one (Preferences ▸ Themes). `StringView::small`
  sets it in the small size.
- `widgets::ThumbnailPicker::new(id, thumbnails, selected)` (pictures
  to pick one from, wrapping in `width`, labelled under each, the chosen
  one on a `style::selection_plate`; a click chooses, a double click
  opens, the arrows move it; `RadioButton`s to agents; `height_for`
  sizes it) with `Thumbnail { picture, color, label }` (a picture made at
  the well's device size). `Cell::icon_text(window, cx, icon, fallback,
  text)`: a list cell's 16-point icon and name, which names its row.
  `image_view::from_bgra` makes a picture of pixels in GPUI's own order
  (a captured frame). A `TextControl` whose field is disabled
  (`TextField::set_enabled(false)`) is drawn faded, label and well, and
  takes no click. `paragraph` says its text to agents.
- Pictures (`RenderImage`s) stay in the GPU's atlas until dropped
  (`cx.drop_image(image, None)`): drop one replaced (a thumbnail that
  refreshes) and those a view keeps when it goes (`cx.on_release`).
- A closed window lets go of its callbacks (and the views they hold) as
  it drops, not once the platform's tidying task runs: a view that
  answers something as it's released (a portal dialog cancelled) does
  so at once.
- What agents read of a window that isn't showing (on another
  workspace, minimized, covered) is as it is now, not as last drawn: it
  draws only when it shows, so the agent socket has each window draw
  what changed (`Window::draw_if_dirty`, which doesn't show it; the next
  frame does) before it answers `tree`, `menus`, `invoke` or `review`.
- `widgets::Sidebar::new(id, rows, chosen).on_choose(f)` (a source
  list filling its box: `sidebar::Row::new(icon,
  label).value(v)` and `Row::heading(label)`, `sidebar::height(rows)`,
  its `ground` and `edge` colours for what sits on it; ↑ ↓ step over
  headings, rows past its height scroll, the chosen one is brought into
  view; `ListItem`s and `Heading`s to agents). `Row::heading("")` is a
  short gap setting what follows apart, said to no one; `.bare()` drops
  its ground and edge, over what's behind it (a list's sunken well:
  Passwords' sources). Several chosen together, the app keeping which:
  `.also(rows)` draws those selected with the chosen one, `.on_pick(f)`
  takes ⌘- (or Ctrl-) and ⇧-clicks (the row and the modifiers),
  `.on_menu(f)` a secondary click (the row and where), and
  `.on_select_all(f)` ⌘A while it has the keyboard; a plain click with
  others chosen chooses that one alone (Agent Screens).
- `widgets::TabView::new(id, labels, selected).on_select(f)` with the
  chosen tab's content as its child (BTabView: slanted folder tabs over
  a raised panel, the content inside `WINDOW_INSET`; `Tab`s to agents).
  Next and Previous Tab (`app::tab_step`: ⌃Tab, ⌃⇧Tab, ⌃Page Down and
  ⌃Page Up) step the window's first tab view from wherever the keyboard
  is, unless what has it stops them (a screen being controlled).
- `widgets::dashboard`: `Card::new(id, title)
  .figure(..).caption(..).selected(..).figure_ink(..).on_click(..)`
  holds its content as children under its head (`Card::height(content,
  caption)`), `Facts::new(rows)` (label: value rows, `.colors(label,
  value)` on a panel), `meter`, `flow`/`flow_rows` for cards placed by
  hand, `GAP`, `FACT_ROW`, `gutter(p)`. Cards in a flex row are as tall
  as the tallest. `style::card(window, r, p, hover, selected)` paints a
  card's paper and shadow; `style::thousands(n)` writes "12,345".
- `menu::shortcut_command(window, cx, keystroke)`: the menu bar command
  a keystroke chooses, for a view that takes the keys itself and lets
  one through (Agent Screens' ⌘K while controlling).
- `code` (source code in the editor; see "Source code" above): `Code`
  (`spans`, `edit`, `reset`, `job`/`done`) and `code::parse`; re-exports
  `olo_syntax`'s `detect`, `by_name`, `Language`, `Edit`, `Class`.
- `editor` (StyledEdit's text; see "The text editor" above): styled
  text in paragraphs, laid out by `editor::layout` (shaped once by
  GPUI's text system, placed in rows: wrapping at spaces and hyphens,
  tab stops left, centre, right and decimal, alignment and
  justification, pictures as characters, emoji). The app keeps an
  `Editor` and its text behind the `Document` trait (`text`, `spans`
  each paragraph's styles, `paragraph` its format: indents, spacing,
  alignment, a list's marker, a table's row, pictures, a shade, bar or
  rule; `replace`, `copied`/`paste` for styles on the clipboard, `key`
  for Return, Tab and Backspace it has its own meaning for, as a list's
  or a table's, `types` for what Return and Tab type instead,
  `link_at` for a link under the pointer)
  and is its `EditorHost` (`editor(key, cx, f)` hands both over;
  `edited`, `context_menu`, `text_moved`, `open_link`: the command key
  and a click on a link opens it, the caret left where it was, and with
  the command key held the pointer over a link is a hand, drawn again
  as the key goes down or up in the focused editor, which is the only
  place a modifier's change reaches, and as the pointer moves with it
  held). `editor::view(host, key,
  &editor, options, window)` is the element: a `ScrollView` that
  scrolls itself, the caret, selection, input methods, the pointer
  (words and paragraphs on double and triple clicks, a selection
  dragged to move it or out to another app, a table's column rule
  dragged), keys (`bind_keys`, in its key context `OloEditor`, bound
  after the window's own so its Tab is its own) and `edit(host, key,
  Edit, ..)` for the Edit menu. `Options`: editable, wrap, alignment,
  justification, margins, background, `spelling` (misspelled words
  underlined, checked on a worker paragraph by paragraph, once until
  they change; `respell` after a word is learnt) and `pages` (set on
  sheets of paper down a desk as they print, rows that don't fit
  moved to the next page; `sheets()` for the app's header and
  footer). Only what shows is laid out, measured ahead a millisecond
  a frame; laid-out paragraphs are kept by their text, styles and
  width, and shaped ones by their text and styles alone, so a new
  width (a window being resized) places them again without shaping;
  each row's glyphs are painted from a `GlyphRun` (gpui/README.md). The
  view is kept on the paragraph at its top through edits above it and
  relayouts; at the very top it stays there (on pages, the first
  sheet's top). On a dark document the text's own colours are made
  lighter until they read (4.5:1, as `olo_theme::readable`; printed and
  saved as they are).
- `widgets::Ruler` (StyledEdit's, after Gobe Productive's and
  ClarisWorks', `ruler::HEIGHT` 26 points high): a paragraph's indents
  (first line, left, right) and tab stops in inches or centimetres
  (`ruler::metric()`, as the locale measures), dragged
  (`RulerEvent::Changed`; snapping to sixteenths of an inch or quarter
  centimetres), a stop added with a click and taken off by dragging it
  away, a double click or a right press for a stop's kind, a line down the text where the marker held goes (`guide()`).
  `set(setting, origin, width)` from the text's place across the
  window, and `follow(origin, width)` from `EditorHost::text_moved` as a
  frame is laid out (scrolled sideways on pages), so it moves with the
  text in the same frame.
- `spell::checker(cx)`: kit-base's checker (`olo_kit_base::spell`, the
  dictionary of the person's language and the words they taught it),
  read on a worker the first time one is wanted; `loaded(cx)` once in.
- Every scroll view's ends are elastic, as on a Mac (GPUI's `Elastic`,
  gpui/README.md): fingers pull past an end and it springs back, a glide
  bounces off it, a wheel stops there. A view that scrolls itself keeps
  an `Elastic` too (`scroll` from its wheel handler with the event's
  `kind` and `touch_phase`, `offset` where it draws, a frame asked for
  while it springs back), as the editor does.
- `workspaces::follow(cx)` (an app that keeps a window per thing on
  each workspace, as Tracker's folder windows): a thread follows where
  the app's windows are (app_server's events for its own windows and
  the workspaces, nothing polled); `workspaces::here(window)` or
  `here_surface(id)` (a window's surface as its view last saw it, from
  outside its update) says whether it's on the workspace the active
  display shows.
- `icons::paint_tinted(window, b, name, tint)`: an icon with its
  colours multiplied (a selected one darkened, as Tracker's), still a
  texture pixel to a device pixel (two of GPUI's image triangles).
- A window is never opened smaller than 1×1: a layer anchored to every
  edge (the desktop) is given 1×1 and app_server sizes it; asked for
  0×0, GPUI made a buffer of nothing, and app_server ended the
  connection.
- `olo_kit_base::clipboard::files`/`copied_files`/`FILES` (files as file
  managers copy and paste them) and `olo_kit_base::files::named_file`/
  `moz_url` (a picture dragged out of a web page, a browser's link)
  are kit-base's, for both kits.
- A single-line `TextField`'s Return is the window's default button's,
  as BeOS's (the field doesn't see it); `keeps_enter()` keeps it for a
  field whose Return means something else (a first password field going
  on to the second). A default button that can't be clicked takes no
  Return.

## The session and in-place reload

Olo upgrades without logging out. `olo-session` (what the login
screen runs) owns the Wayland socket (`wayland-N`) and the IPC socket and
hands both to app_server, so clients never see the sockets disappear.

**The displays' modes at a restart** (`display.keep_modes` in
app_server.json, or `OLO_KEEP_MODES=1`; off unless set, until tried on
the real displays). As ever, app_server opens each GPU with every
connector switched off (Smithay's `reset_state`) and modesets each
display from dark, and the app_server going puts back the state it
found: each display dark a second or two, twice, at every upgrade.
Kept, in Olo's Smithay (`backend::drm::keep_current_modes`): the
device isn't reset, only its cursor and overlay planes switched off in
a commit that needs no modeset (refused, it's reset as ever); a surface
whose CRTC already shows the mode asked for (`same_timings`: a CRTC's
mode and its connector's listed one differ in their type bits and
name) on the connectors asked for starts from that state, so its first
commit is a page flip; and the app_server going leaves its displays
as they are for the next. Rescue modesets as ever.

While a restarted app_server brings the windows back, every display shows
the **upgrade's veil** (app_server's `veil.rs`), an homage to BeOS's boot
screen: Olo's wordmark on dark and a row of icons from the set, dim until
their part is done and lit in turn (the upgrade, the displays, windows
coming back, all of them), with a line saying which. It starts only
after a restart with windows to bring back (`layout::load` found places
saved moments ago; not in Rescue, not while locked, `OLO_NO_VEIL` to
skip it), shows at least 0.9 s (so it's seen, not flashed: windows can be
back in a third of a second), lifts once as many windows are back as
were saved (or after 6 s) and fades over 0.4 s. A solid background and,
centred, a picture drawn once per display, scale and stage with
tiny-skia: the wordmark pre-scaled offline to each scale's width
(`data/brand/sizes`, 256 points: 256 to 512 pixels) and the icons at 64
points from the set's sizes, both pixel for pixel (between-scales take
the next size down, centred: never resampled); a frame only as a stage
is reached and through the fade. `OLO_VEIL_SHEET=/var/tmp/veil.png cargo
test -p app_server --lib veil_sheet` draws its stages at 1, 1.5 and 2×.
The same is **Olo's start-up screen** as a session starts: olo-session
tells its first app_server (`OLO_STARTING`, in a real session; a test's
with `OLO_STARTUP_VEIL=1`), which shows "Starting Olo" with its own
icons (the window server, the displays, the desktop, the apps), lit as
the displays come up, Tracker's desktop layer shows and Deskbar's does
("Welcome"), at least 1.8 s, then faded. As it starts, the session's
Notifications plays **Olo's start-up sound**, once a session (a mark by
the session's pid; not when re-executed or started again) unless
Preferences ▸ Sound's "Play a sound when Olo starts" is off
(`Settings::startup_sound`), one of two made in process
(`notifications/src/startup.rs`, `Settings::startup_chime`): "Olo
saying its name", the default (Ben, 2026-11-06: "like \"ooollllooooo\"
reminiscent of the mac startup sound but saying olo"), a start-up
chime's bright F major chord over three octaves (each note two
band-limited saws a few cents apart) voiced by three formant resonators
gliding "ooo", "lll", "ooo" (the chord dipping for the "l" and blooming
again, its pitch easing down a little through it, as a voice's), 3.8 s;
or a swell, a D♭ major ninth chord coming in low to high, each note a
sine with a little of its octave and twelfth, 3.2 s. Either spread in
stereo with a light Schroeder room, at −6 dBFS, written to the cache as
a WAV and played as Notifications' sounds are. Preferences ▸ Sound's
Play beside the choice runs `olo-notifications --startup-sound
olo|swell` (beside Preferences), which plays one and exits;
`olo_says_o_l_o` checks the "l" moves the chord's energy from the
"o"'s second formant (700–950 Hz) to its own (1050–1300 Hz) and back
(`OLO_STARTUP_WAV=FILE cargo test -p olo-notifications startup_wav`
writes both out to listen to). Before the
session, the boot shows the same picture (Plymouth's theme) and Olo's
own login screen asks who: "The login screen and boot splash" below.
Their connections do end when app_server restarts in place, and each
kind of client has its way back: an Olo app re-executes itself with
its state saved, as for an upgrade, and reconnects (olo-ui
`upgrade::lost_compositor`, from GPUI's `ON_COMPOSITOR_LOST`: after each
turn of its loop GPUI asks the connection's socket whether it hung up
and the connection whether it holds a protocol error, since calloop
drops a failing source and the loop would sleep on with its windows
gone, as Deskbar did, or spin on a stored protocol error, as
Notifications did, 2026-11-01; once told, the loop stops, so a program
nobody re-executes, outside a session, ends rather than waking for ever
on the dead socket); GPUI's connect waits up to 15 s for
the compositor to be back. And it connects to app_server's direct
socket (`olo-<display>-direct`), not to
`WAYLAND_DISPLAY`, which in a session is olo-wlproxy's: the proxy keeps
outside programs' windows across a restart by replaying their objects
on the new app_server, and a GPUI app replayed that way kept its old
configures, outputs and frame state and was left with dead windows.
Test it as the real thing happens, displays coming up late and
nameless after the restart:
`OLO_HEADLESS_LATE_MS=2500 OLO_HEADLESS_ANONYMOUS_MS=2500 tsession start
NAME 2560x1600,1920x1080 --session --shell`, start apps in it,
`tsession reload NAME` (twice), and every app is there after, each
display with its own Deskbar (`tsession shot NAME out.png --output
HEADLESS-2` for the second), no "exited" or protocol error in
`/tmp/olo-dev/NAME/app_server.log`. A bar for a display this app's
connection doesn't know yet is refused by GPUI (not stood in for by the
compositor's choice, which put the Dell's bar on the Studio Display)
and asked for again from `olo_ui::on_displays_changed`.

What an upgrade leaves behind is looked at (`Session::check_stale`,
Ben, 2026-11-01): 20 s after the programs were asked to re-execute,
those of this session still running a replaced binary (`/proc/PID/exe`
"(deleted)" under the install directory, `WAYLAND_DISPLAY` ours) are
named in the log and asked again; 90 s after, what is still old is
stopped: a service of the session's is restarted by it, an app starts
as the new version when next opened. The proxies are never touched
(a retired one serves the outside programs it had until they close),
nor olo-portal (it may be waiting out a screen share,
`upgrade_waits_while`), which is only named, nor olo-pool (one from
before the waiting marker drains, then becomes the new version by itself
within its own 10 minutes), nor a program marked waiting for what it has
under way (`olo_kit_base::restore::mark_waiting`). Programs of ours left
from a session that has gone (their `WAYLAND_DISPLAY`'s socket no
longer exists: a login before, a test session or Agent Screen stopped
without them; `upgrade::leftovers`, none younger than 30 s, none with
no display in their environment) are asked to end at the first look
and killed at the second if they didn't (`sweep_leftovers`, by pid and
start time; only by the person's session). Nor is stopped a program of ours that
another program started (`upgrade::started_by_another`: its parent is
neither ours, the session's service manager nor init): a coding agent's
`olo-terminal --mcp`, a browser's messaging host, a command run from a
shell. Stopped, what that program had open with it broke and it never
started it again (Claude Code's tab-title tool was gone for the rest of
its session, 2026-11-05).

Login items (`session/src/autostart.rs`, the freedesktop Autostart
spec): once a real session's graphical side is up (the environment
exported, the portals set), olo-session starts what the person asked
to start at login, `~/.config/autostart/*.desktop` (Slack's and
Discord's "Launch on Login", what the Background portal writes), each
with `open::launch_line` (field codes dropped, quoting kept). A
person's entry overrides the system's of the same name (`Hidden=true`
turns one off); entries hidden, turned off
(`X-GNOME-Autostart-enabled=false`), not for Olo (`OnlyShowIn` without
it, `NotShowIn` with it) or whose `TryExec` isn't installed don't
start. The system's (`$XDG_CONFIG_DIRS/autostart`) start only when
their `OnlyShowIn` names Olo: Olo says it is GNOME too, and GNOME's
settings daemons, keyring components and applets would otherwise start
and fight Olo over power, keys and displays (none of Fedora's name
Olo). A session resumed after an upgrade doesn't start them again.

App scopes (`xdg/src/scope.rs`): a program started from a desktop entry
(Deskbar, the palette, opening a file, Open With, login items) goes in a
systemd scope of its own, `app-olo-<entry id>-<pid>.scope` (the id
escaped as systemd does, `google\x2dchrome`), asked of the person's
systemd over D-Bus (`StartTransientUnit` with its pid,
`CollectMode=inactive-or-failed`, so it goes when the program does)
just after it starts, on a thread. xdg-desktop-portal reads an
unsandboxed program's app id from that name, so the portals (screen
sharing, Inhibit, GlobalShortcuts, notifications, Account) know which
program asks. Use `open::spawn_app(argv, id)` or
`spawn_detached_app(cmd, id)` for a desktop entry's program; Olo's own
programs (`olo-*`) and programs run in Terminal stay in the session.
Chromium and Electron programs (Chrome, Slack) move themselves into a
scope of their own as they start (`app-com.google.Chrome-<pid>.scope`,
`app-slack-<pid>.scope`), which wins. Where there's no systemd (a test
session's bus) the call fails quietly and the program stays where it
was. `cargo test -p olo-xdg scope -- --ignored` makes one real scope,
for a 3-second `sleep`.

* **Other programs** (Chrome, GTK and Qt apps, terminals) connect to
  `wayland-N`, which belongs to `olo-wlproxy` (`crates/wlproxy`).
  It opens a connection to app_server for each of them on
  `$XDG_RUNTIME_DIR/olo-wayland-N-proxied`, which begins with a 16-byte
  hello naming the program's pid. app_server answers with its own pid,
  and uses the program's pid everywhere it would read the socket's
  (`state::client_pid`). The proxy decodes every message and records
  each object's lasting state: how it was made, setters that still hold,
  and a surface's committed buffer, scale and regions. It also keeps each
  buffer's recipe, with descriptors.
* When app_server goes away, the proxy tells each program what a
  restart ends: pointer and keyboard leave, frame callbacks fire, popups
  are dismissed, selections are cancelled. The program's requests wait.
  On the next app_server the proxy remakes the objects with the same
  ids, matches globals by interface and order, commits each window's
  role and acknowledges its configure, then commits the contents. The
  waiting requests go on after that. The program never notices, and the
  window returns to its place and depth (same pid, new client;
  `Placement::depth` keeps the stacking whatever order programs
  reconnect in: `layout.rs`). Objects app_server made (offers, buffers
  it sends: ids from 0xff000000) die with it; the program's ids for the
  next app_server's are the same unless the program still holds that id
  (`server_id`), and requests are translated before the proxy's books
  change (a destroy of a remapped offer went up under the program's id,
  one app_server never made, and the program was cut off with "invalid
  object": GTK's editor at its first selection after a restart).
* The proxy shows programs only the globals it can decode
  (`wlproxy/src/interfaces.rs`); add a protocol there before an outside
  program can use it. That is every global app_server has for programs
  except four it keeps from outside ones: the session lock, screen
  capture (screen sharing will come through a portal), security contexts
  (Flatpak's sandboxes come through the proxy instead) and explicit sync
  (a restart can't keep a buffer's release points). Clipboard tools and
  password managers (1Password: arboard → wl-clipboard-rs) need data
  control (`ext_data_control_v1`, `zwlr_data_control_manager_v1`), so
  they copy and paste even without a focused window. Without it (a
  proxy from before data control, still serving programs started
  before an upgrade), wl-copy and wl-paste make a 1x1 window to get the
  keyboard with; app_server keeps those (`shell::xdg`
  `CLIPBOARD_HELPERS`, `clipboard_helper`) out of the space: never
  drawn, tiled (auto-tiling gave them a tile, squeezing the window a
  selection was being copied from), decorated or listed, with the
  keyboard for their moment and handing it back. GTK asks for
  server-side decorations through `org_kde_kwin_server_decoration`, input
  methods use `zwp_input_method_v2`, and idle notifications track
  app_server's own idle timer. Requests whose effect lasts (`slot()` in
  `client.rs`: a dialog's modality, a surface's alpha or content type, a
  virtual keyboard's keymap) are replayed after a restart. Objects the
  restart ends get their end events (data sources are cancelled, drm
  leases finish, tablets are removed). What a restart must not do to a
  program (each one ended Chrome, Slack, the GNOME portal and every GTK
  and Qt program at upgrades, 2026-11-04 and -05):
  - A touch (`wl_touch`) waits for the new seat to say it has touch
    (`waiting_touch`, made again in `seat_capabilities`): a new
    app_server's seat gets its touch screen only as libinput finds it,
    seconds after it starts, and Smithay ended the connection of a
    program asking before ("missing touch capability"). A program told
    the seat has none lets its touch go and asks again when it does.
    Smithay (ours) also gives an inert pointer, keyboard or touch to a
    request without the capability rather than ending the connection.
  - A surface whose destroy is waiting in the queue isn't made again,
    nor anything made from it; a fractional scale whose surface the
    program let go (destroyed, its destroy waiting, its id another's)
    hears nothing more (`surface_gone`); a surface shown nowhere yet (no
    role, never committed, never a cursor or drag icon) gets its scale
    only when it is (`held`, `release_held`); a scale the program was
    already told isn't said again (`told_again`); and the replay commits
    only surfaces the program committed. GTK keeps a removed tablet's
    cursor surface's `wp_fractional_scale_v1` while freeing what its
    events land on, and told the tablet went (it comes again from the
    next app_server), it crashed on the next app_server's scale
    (`pointer_surface_fractional_scale_preferred_scale_cb`), however late
    it read (Chrome's GTK sat 25 s in a call to the accessibility bus).
  `tools/dev/outside-check` runs GNOME Text Editor (GTK 4), Slack
  (Electron), Chrome (olo-chrome, a profile of its own) and a Qt 6
  window (`OLO_QT_PYTHON`, a Python with PySide6) in a session under
  olo-session at 1.5×, with a touch screen that comes 2.5 s after each
  app_server starts and a pen tablet (`OLO_HEADLESS_TOUCH=MS`,
  `OLO_HEADLESS_TABLET=1` for any headless session), restarts app_server
  N times and checks each is running with its window (before the fixes
  all four died at the first; after, all four through five). Run it
  outside a coding agent's sandbox: Chromium's own can't start inside
  one. The proxy's CPU for 704 keys typed into GTK: 150–210 ms before,
  160–200 after. Set `OLO_WLPROXY_DEBUG=1` for
  the session (`tsession` passes it on) to log every replayed request. Set
  `OLO_NO_WAYLAND_PROXY=1` to have app_server take `wayland-N` itself.
  If the proxy keeps crashing, the session does that on its own.
* **Moving a program to another app_server** (an agent screen's, or from
  one to the person's): each proxy listens on
  `$XDG_RUNTIME_DIR/olo-wlproxy-PID.sock` (`wlproxy/src/control.rs`, a
  line a request: `list`, `move PID PATH`, `home PATH`). `move` points
  the program's connection at that app_server's proxied socket
  (`Client::move_to`): the program is told what a restart ends and
  replayed there, the same pid, so it never knows (GNOME Calculator moved
  in 0.1 s, its 68 objects replayed). A program moved away whose
  app_server has nobody listening any more (its session gone: a session
  removes its sockets as it ends; a restart keeps them) comes back to
  the proxy's own after 2 s (`AWAY_GONE`); `home PATH` brings back at
  once every program on that app_server (a screen stopping). A screen's
  proxy told to end (SIGTERM, `OLO_SCREEN` set) while it has programs on
  the person's display forks: the parent exits at once (the session
  isn't kept waiting) and the child, in a session of its own with a
  control socket under its own pid, serves those programs until they
  close. The person's proxy ends with their session as before.
* **Upgrading the proxy** (`Session::renew_proxy`): restarting it would
  close every outside program's windows, so it isn't. When an upgrade
  brings a new `olo-wlproxy` (the session notes the program it started
  the proxy from: inode and mtime), the session binds a fresh listening
  socket and renames it over `wayland-N` (a program connecting meanwhile
  finds one or the other), starts the new proxy on it and sends the old
  one SIGWINCH. The old one stops accepting and serves the programs it
  has until the last one closes, then exits; the session reaps it
  quietly (`retired`, kept across its own re-execution). SIGWINCH is
  ignored by default, so a proxy from before this neither dies nor
  exits: it keeps its programs until the session ends, unreachable by
  new ones. New programs get the new proxy, and its protocols, at once.
  The same hand-over happens when the session makes `wayland-N` again
  after something removed it (before, the proxy kept listening on the
  removed socket). `tsession` follows a session whose Wayland socket
  changed while its IPC socket and pid didn't. Test it with a sandbox
  install (below): an old install, a new tree, an outside program open,
  `kill -USR2` the session, then check the program kept its window and a
  new one goes to the new proxy (its descriptors); upgrade the proxy
  again and close the programs to see the retired one exit.
* **Olo's own apps** connect straight to app_server on
  `$XDG_RUNTIME_DIR/olo-wayland-N-direct` (GPUI's `connect`). They
  re-execute and reconnect by themselves, as below.
* Test with `tsession start NAME --session`. Run an outside program in
  it with a fixture `HOME` and a private bus (`tools/dev/busd-lenient`),
  then run `tsession reload NAME` a few times and check it still draws,
  takes input and opens menus.

* `olo-session reload` (SIGHUP): if `olo-session` itself was
  upgraded it re-executes the new binary first, keeping its children and
  sockets, with the arguments it was started with (`start_args`, made
  from what it keeps: its backend's, `--greeter`, a screen's
  `--own-bus`; the new self resumes and doesn't read them, but
  olo-screen knows a screen's session by its `--headless`, and every
  screen re-executed bare looked ended, so starting it again by name
  started another, the old one left running unknown: nine on
  2026-11-07). Resumed, it stops the children this version doesn't run
  (`retire_dropped`, by program: `SHELL`, `REAL_SHELL`, a screen's bus
  and services, the greeter), rather than starting them again each time
  they end: agent screens from before their portals were Olo's alone
  kept GTK's portal backend going, so Olo Doctor's Quit Them never
  held. Then it stops app_server (SIGTERM; it saves every window's
  place to `$XDG_RUNTIME_DIR/olo-layout-<display>.json`) and starts the
  one now on disk, which puts returning windows back (same app id and
  title within 60 s; position, size, workspace, minimized, focus).
  `layout.rs` decides what is returning: the same process on a new
  connection (title, else app id, within a minute), or an app started
  again after its old process ended, with the very same app id and title
  within 15 minutes (Chrome restarting restores its windows; closed
  windows are remembered in memory, up to 200, but not one closed by
  hand, which Olo asked to close: `WindowMeta::closed_by_hand`, so a
  new "Untitled 1" opens where the person is, not where the last one
  was closed). An app started again
  gets the place back but not a group the window was put in by hand
  (`Grouping::With` becomes `Auto`): it is another window with the same
  title ("Untitled 2"), and would join another app's windows. The place is claimed at
  the window's first commit, so its first configure already has the old
  size, and applied when it maps. A window of its own size (not tiled
  or zoomed) back on the display it was saved on, unchanged
  (`Placement::screen`: its name and place), is put back exactly; only
  after the displays changed is it fitted to them again (`refit`).
  Fitting it anyway squeezed a window bigger than the room beside
  Deskbar (which may not even be back yet) and moved it, as GTK's
  windows on a small screen were; tiled and zoomed windows are
  put back into their tiles (auto-tiling's trees are kept: "Windows" ▸
  auto-tiling; a tiling command's tiles are refitted into their parts),
  and a parked window's place is saved on the display it left
  (`display_of`), as it comes back. A window stopped in between (claimed,
  not shown yet) is saved at the place it claimed, not the cascade slot
  it waits in (`placement_of`). Each display's workspace showing comes
  back too (`Placement::displays`, by display key, `layout::showing`;
  a layout saved before each display had its own has one for them all,
  `Placement::showing`, kept under `layout::EVERY`). Everything else is a new window and
  opens on the display in use (`new_window_near`: beside the app's
  focused window, else on the active display, the one apps are told is
  in use: under the pointer if it's there, else in the middle), including
  a running app opening a window again.
* Olo's apps notice the lost connection (olo-ui's
  `upgrade::lost_compositor`), save their state (`olo_ui::on_upgrade`),
  re-execute themselves (same pid and arguments) and read it back with
  `olo_ui::restored_state()` at start-up; GPUI's connect waits up to
  15 s for app_server to be back. An upgrade of an app's own binary
  does the same when olo-session signals it (SIGURG), unless
  `upgrade_waits_while` says it is doing something re-executing would
  cut short (olo-portal sharing a screen): it becomes the new version
  once that turns false. `on_upgrade_due` says a new version is waiting,
  for an app to stop taking new work and finish what it has (olo-pool's
  drain). In an upgrade olo-session signals every program it replaced
  and restarts app_server at once, so an app re-executes while the old
  app_server goes: one may start against the old one and lose it in its
  handshake, and simply goes round again. No app may die of an upgrade.
* Nor of losing the GPU (a driver reset: `VK_ERROR_DEVICE_LOST`, as
  Asahi's Mesa gives at a failed syncobj transfer): everything the
  device made is gone (pictures, pages, glyphs), and only the app makes
  them again. Blade marks the device lost where a submit, present, wait
  or acquire finds it (gpui/README.md), and the app starts anew as for
  the compositor gone (`upgrade::lost_compositor`: its state saved,
  re-executed, same pid). Lost again within 10 s of that start
  (`OLO_GPU_LOST_AT`, taken out of the environment as it starts), the
  GPU is broken, not reset, and the app ends rather than go round. Test
  with the app socket's debug-only `{"lose_gpu":true}`
  (`olo-menus-PID.sock`), which marks the device lost.
* A re-executed app keeps its pid, so the children its previous self
  started are its own still, but nothing in it waits for them: Activity
  Monitor's helper, its input pipe closed with the exec, ended and
  stayed a zombie, one an upgrade. `restore::claim` (the first thing
  `olo_ui::run` does) has a thread wait for whatever children the
  process has as it starts.
* app_server puts a window back on its own as it was in every way: a
  window remembered by this app_server whose own process came back
  (`Placement::id`) is stacked among the others by the windows that
  were under and over it (taken once as its client's windows begin to
  go, `closing_stack`; found again by `WindowMeta::was`), and the
  keyboard and the active window it took as it opened go back to what
  had them (`WindowMeta::focus_before`). Deskbar keeps a gone app's
  place in its list for 20 s, and saves its order to re-execute with.
* **Every app with windows the user
  arranged must implement both**: save which windows are open (with
  their sizes), their documents/folders, view settings and
  selection, and reopen them in the same order. Tracker's `Saved` is the
  example.
* app_server or a shell component that crashes is restarted (up to 5
  times a minute). `olo-session quit` ends the session.
* olo-session's watchdog (`crates/session/src/watchdog.rs`) pings
  app_server over IPC every 2 s. After 10 s without an answer it sends
  SIGABRT (so `coredumpctl debug` has the stack that hung), then SIGKILL
  10 s later, and restarts it like a crash: apps reconnect and a locked
  screen comes back locked.
* Never replace an installed binary by writing over it (install to a temporary
  name and rename, as `tools/install-session.sh` does).

On a fresh Fedora machine (Fedora 44 or 45, x86_64 or aarch64,
Workstation or atomic), `tools/setup.sh` says what it's on (the release,
processor, memory, free space, graphics and login screen), installs what's
missing (its `NEEDS`: compilers and libclang, the libraries linked,
PipeWire's headers, Git LFS, Mesa's DRI and Vulkan drivers and the
Vulkan loader, the session's services, the fonts, Pillow for the dev
tools; each checked first, so a remix's own builds count), fetches Git
LFS's files (the voice model is built in), installs Rust through rustup
if there's none new enough (`RUST_MIN`, the newest `rust-version` a crate
asks for), builds and installs (`tools/install-session.sh`: GDM's and
SDDM's session menus), and turns on the sweep, the hooks and kache's
daemon; `--check` only reports. Every package it names was checked
against Fedora 45's x86_64 repository (2026-11-03). `tools/dev/x86-check` type-checks the workspace for x86_64
from an aarch64 machine (x86 headers unpacked under ~/.cache; nothing
installed).

Install for the login screen with `tools/install-session.sh` (as
yourself; it asks for sudo once). It builds release binaries into
`~/.local/bin` (so upgrades need no root), icons into
`~/.local/share/olo`, and the session entry into
`/usr/share/wayland-sessions/olo.desktop` (Exec is the installed
path; no TryExec, as the greeter can't look inside a private home):
where GDM and SDDM always look, as GDM reads
`/usr/local/share/wayland-sessions` only when its `XDG_DATA_DIRS` names
it (on Fedora a Flatpak's environment generator does; a machine without
one listed no Olo). On an atomic system, whose `/usr` can't be written,
it goes in `/usr/local/share/wayland-sessions`; an entry in the other
place is removed. `OLO_SKIP_UI_LINT=1` leaves out the live layout lint
before installing (`tools/setup.sh` does; the hooks run it).

**Upgrades from the build tree** (`crates/session/src/upgrade.rs`): the
installer records the tree's `target/release` in
`~/.local/share/olo/build-dir` (`OLO_UPGRADE_FROM` overrides it).
Every 3 s the session compares the `olo-*` programs there with its
own. It skips this while cargo holds one of its locks in the tree
(`.cargo-lock`, or `.cargo-build-lock`/`.cargo-artifact-lock` in newer
cargo) or a program is under 3 s old. It lists newer ones in
`$XDG_RUNTIME_DIR/olo-upgrade-<display>.json`; Deskbar follows that file
(`olo_ui::watch`) and shows a green arrow in its tray. Clicking it asks first. Accepting,
or `olo-session upgrade` (SIGUSR2), installs them beside the session:
a btrfs/XFS clone (`FICLONE`) or a copy, under a temporary name and then
renamed. Only a new `olo-app-server` needs a reload. Other programs
keep their windows through it (above) unless they came without the proxy
(`WindowInfo::kept` is false), and Deskbar's alert names those first. Otherwise app_server keeps running: the session
sends SIGURG to each of the user's processes whose program file in the
install directory was just replaced (`/proc/PID/exe` ends in
" (deleted)"). The kit catches it (a self-pipe in `platform.rs`), saves
state and re-executes, as after a reload, and app_server puts its windows
back where they were. A program with no kit loop takes it with
`olo_kit_base::restore::on_upgrade` (a self-pipe and a thread):
`exec_on_upgrade` re-executes with the same arguments (`olo-doctor
--watch`, whose state is on disk); one with a poll loop of its own takes
`upgrade_signal()` (a file that becomes readable) and calls
`exec_new_version()` when it's between things, and the Chrome extension's host
(`olo-chrome chrome-extension://…`) exits once no request is being
answered, for the extension to start the new one. Whatever is still
old 90 s after is stopped (`check_stale`). A new olo-session re-executes itself without a
reload. When app_server is new too, it restarts first and the nudge
comes `NUDGE_AFTER_RELOAD` (6 s) after, to what is still old: kit apps
re-execute as their compositor goes anyway, into their new versions.
Nudged together with the reload (as before 2026-11-04), an app
re-executed twice over and saved its windows half gone: Terminal's came
back in another order, in other tiles. `tools/dev/order-check` does an
upgrade so (a reordered Terminal group and StyledEdit, auto-tiled) and
compares each window's tab place and tile. A window not shown yet still
keeps the place it claimed, and the kit survives a lost surface
(above), for a program restarting while app_server does. Test it with
`TSESSION_SESSION_EXE=<install>/bin/olo-session
OLO_UPGRADE_FROM=<tree> tsession start NAME --session --shell` and
`touch` a program in the tree. Make the install and the tree under
`/var/tmp` (`cp --reflink=always` the release programs into
`<install>/bin`, then again into `<tree>`), not `/tmp`, which is RAM.
For the race: touch an app's program, send the app SIGURG, then send
the session SIGHUP 30–120 ms later, and check the window kept its
place.

**Agent Screens follow the person's upgrade.** A screen is a headless
olo-session of the installed programs; it installs nothing (that's the
person's session, into the same directory), so its own watcher never
sees anything newer. Instead it notices what was replaced under it
(`follow_install`, on its 2 s tick: its app_server's `/proc/PID/exe`
"(deleted)" under the install directory, its own binary or the proxy's
changed) and, once that has held `FOLLOW_SETTLE` (4 s, the install
done), does what its own upgrade would: app_server reloaded in place
(apps re-execute as they reconnect, nudged after), itself re-executed,
the proxy renewed, what's left old asked after. Before (2026-11-06) a
screen ran its old version until stopped: its Live view still blacked
Passwords out after the upgrade that ended that. A screen whose session
is from before this can't follow, so the person's session reloads it
(SIGHUP) at its 20 s look (`Watcher::old_screens`). Only the person's
session sweeps leftovers; one run from the build tree (tests) never
follows a build. To try it: copy olo-session, olo-app-server and
olo-wlproxy into `/var/tmp/X/bin`, start `/var/tmp/X/bin/olo-session
--headless 800x600 --no-shell` with `OLO_UPGRADE_FROM` set (and the
person's `WAYLAND_DISPLAY`, `OLO_SOCKET` and `OLO_SESSION*` unset), then
replace one of them (copy to a temporary name, `mv` over): the log says
"following" and the new pid runs the new file.

**Finding flicker** (headless): screenshots take about 50 ms, too slow
for a flash a frame long. `tsession ipc NAME
'{"type":"record","path":"/var/tmp/DIR","frames":150}'` makes
app_server draw what the screen shows after each of its next 150 turns
(every client request handled, not only each frame) and save each
picture unlike the one before as `NNNN-MS.png` (milliseconds from the
start), on a thread; then do the thing (`tsession key NAME 57`) and look
at the pictures in order. That is how Quick Look's flash showed: a frame
of it in its cascade slot before it was over Tracker. Windows are
placed (back where they were, or over their parent) in the commit
handler once their first buffer is in (`place_on_content`), not in a
post-commit hook: smithay runs those before the commit handler takes
the buffer in, so the window had no content yet, was placed a commit
late, and showed a frame elsewhere first. A tiled or zoomed window its
app keeps bigger than its tile (its least size) gives way inwards on
commit, staying in the tiling area (`keep_in_tile_area`).

**The login keyring** (`crates/session/src/keyring.rs`): PAM starts
gnome-keyring-daemon with the login keyring unlocked, and the daemon
quits after two minutes unless the desktop finishes starting it. GNOME
does that with `gnome-keyring-daemon --start`; a real session sends the
same INITIALIZE over `$XDG_RUNTIME_DIR/keyring/control` (big-endian
length-prefixed fields: op 0, components `secrets`, the environment) and
takes on the environment it returns. Otherwise the first program to want
a secret starts a fresh daemon that asks for the password again.

**keyd** (`crates/session/src/keyd.rs`): keyd remaps keyboards for every
session, so a real session asks it over `/run/keyd.socket` to bind the
modifier keys and Caps Lock to themselves. That way app_server's
keymap.rs sees the real keys. The session repeats this every minute,
because `keyd reload` forgets binds, and sends `reset` when it ends.

**New hardware**: `olo-app-server --probe-gpus` lists the DRM devices,
their KMS resources and the render node their EGL uses, without
disturbing a running desktop. On Apple silicon the boot GPU (asahi) has
no displays. The display controller (apple-drm) renders through it via
Mesa's kmsro, so a primary GPU without modesetting is skipped rather
than treated as fatal. Under GDM the session's output goes to the journal
(`journalctl -b | grep olo`). If app_server can't start, the session
gives up after 5 crashes a minute and the login screen comes back;
Ctrl+Alt+Backspace quits the session.

### Renamed from Tabula

Olo was called Tabula until its rename; installs and running sessions
from then carry on. `olo_xdg::legacy`
holds the bridge, and the old name is spelled nowhere else but the
places below.

* `legacy::adopt()` is the first line of every program's `main` (apps,
  termd, app_server, the session, wlproxy, op): `TABULA_*` variables
  become `OLO_*`, and `tabula` under the config, cache, state and data
  folders moves to `olo`, leaving a `tabula -> olo` link. New programs
  call it too.
* `legacy::runtime_file(dir, "olo-…")` finds a running Tabula session's
  `tabula-…` sockets, locks and files in `$XDG_RUNTIME_DIR` (the IPC,
  direct, proxied and termd sockets, pid and layout files, agent screens);
  use it for any runtime path another program may have made.
* The session crate is a library with two programs: `olo-session`, and
  `tabula-session` (`src/old_name.rs`), which is what a Tabula session's
  upgrade watcher offers (it only looks for `tabula-*`). Accepting it
  installs every `olo-*` program, turns each old `tabula-X` into a link
  to `olo-X` (`upgrade::link_old_names`) and re-executes as Olo, then
  reloads and nudges (`Resume.nudge`) so every app and termd re-executes
  too. app_server maps saved `tabula.X` app ids (`legacy::new_app_id`),
  so windows come back where they were.
* Kept on purpose, forever: the keyring entries Passwords and the Token
  Pool find their keys by (`application` = `tabula-passwords`,
  `tabula-pool`); Chrome's old native-messaging host
  (`org.tabula.passwords`, written beside `com.olodesktop.passwords`
  for an extension not yet updated); `tabula-*` program links; and
  readers for old data: StyledEdit's `user.tabula.*` attributes,
  Viewer's `tabula-` annotations, `.tabula-backup` files, the Codex
  provider `tabula_pool`, `tabula-termd` and `tabula-session`
  processes, and agent screens' `tabula_socket`.
* `tools/install-session.sh` moves the folders and links the old names
  the same way, installs `olo.desktop` and removes `tabula.desktop` and
  the old udev rule. The login screen offers "Olo Desktop" after that.
* To test: install a pre-rename build into a fixture HOME, start a
  session on it (`TSESSION_SESSION_EXE=…/tabula-session`), open windows,
  then point `OLO_UPGRADE_FROM` (`TABULA_UPGRADE_FROM` for the old
  session) at this tree and `kill -USR2` the session.

## The login screen and boot splash (apps/login, apps/greeter, crates/greet; app_server plymouth.rs)

Olo's own login screen, in GDM's place, and its boot splash, in
Plymouth's theme's. `tools/install-login.sh` installs the login screen
beside GDM (programs into `/usr/local/libexec/olo`, Olo's data into
`/usr/local/share/olo`, its PAM services, user and unit); `--switch`
makes it the login screen from the next start and `--revert` gives GDM
back; `--plymouth` installs the boot splash and `--revert-plymouth`
takes it out. What the machine starts into is the person's to choose:
nothing switches without those.

**The pieces.**

* `olo-greet` (crates/greet): what they say to each other, a JSON line
  each way. The greeter's `Request`s (`Users`, `Sessions`, `Begin`,
  `Answer`, `Start`, `Cancel`, `Power`) and the daemon's `Event`s
  (`Prompt`, `Failed`, `Authenticated`, `Started`, …) on
  `/run/olo-login/greeter.sock` (`OLO_LOGIN_SOCKET`; only the greeter's
  user may connect); a worker's `ToWorker`/`FromWorker` on its pipes.
  `pam.rs` is PAM through the dlopen'd libpam, its conversation asked
  of a closure (`pam_start_confdir` for a test's own stack);
  `users::all` the people who can log in (`/etc/passwd`'s, with
  AccountsService's pictures) and `sessions::all` the
  `wayland-sessions` (Olo's first).
* `olo-login --daemon [--programs DIR] [--vt N] [--test DIR]`
  (apps/login, root): the seat's terminal, the greeter's session, and
  for each login a worker per way in, all at once (`--worker WAY USER`:
  the password through the `olo-login` PAM service, a fingerprint
  through `olo-login-fingerprint`, the face through `olo-login-face`,
  `data/pam`); the first way in wins and the others stop. The winner
  opens logind's session through PAM (`pam_systemd`) and becomes the
  person (setsid, the terminal, their groups, gid, uid) to run their
  session once the greeter has gone. The greeter's own session is a
  worker too, as the `olo-greeter` user (`data/sysusers`) through
  `olo-login-greeter` (in without a question), running `olo-session
  --tty-udev --greeter`; it comes back when it ends and when the person
  logs out.
* `olo-session --greeter`: app_server and `olo-greeter` alone, no
  upgrades watched; the greeter quitting ends it.
* `olo-greeter` (apps/greeter): a layer surface on every display, the
  first holding the keyboard: the person's picture, who (a choice when
  there are several), their password, what the ways in say ("Place your
  finger on the reader"), the session, and Sleep, Restart and Shut
  Down. It remembers who logged in last and their session
  (`$XDG_STATE_HOME/olo/greeter.json`, in `/var/lib/olo-greeter`).
* **Plymouth handed over as GDM does** (apps/login `plymouth.rs`,
  Plymouth's own protocol on its abstract socket): told to let go of the
  screen before the greeter's session takes it, then, once the greeter
  connects, to quit leaving its picture there, so no console shows
  between; told to quit outright (the console back, saying why) if the
  greeter's session ends first or hasn't shown in 30 s. Its unit
  conflicts with `plymouth-quit.service` and comes after it, as
  `gdm.service` does.

**On a stock Fedora, as GDM is** (each found in a fresh Fedora 45 in a
virtual machine, Olo Setup's test, 2026-11-07, and each one kept anyone
from logging in):

* **The first terminal its own.** Fedora 45 runs its text consoles in
  kmscon (`kmsconvt@tty1.service`, `autovt@`'s alias): the unit
  conflicts with it and comes after it, as with `getty@tty1.service`
  (GDM's unit has both). Left there, kmscon kept the terminal, and the
  greeter's session found it gone.
* **What sessions say goes to the journal**, not the terminal (a
  stream to journald's stdout socket, `worker.rs` `journal_stream`,
  each named for its program: `journalctl -t olo-session`), as GDM's
  sessions' does: writing to a terminal taken away failed and ended
  olo-session and app_server (Rust's `eprintln!` stops a program when
  it can't write).
* **A display manager to SELinux.** Its program is labelled `xdm_exec_t`
  so it runs as `xdm_t`, as GDM's daemon does (`/usr/sbin/gdm`); its
  socket's folder `xdm_var_run_t` and the greeter's home
  `xdm_var_lib_t`, as `/run/gdm` and `/var/lib/gdm` are (recorded with
  `semanage fcontext` and put on with `restorecon`: install-login.sh,
  and Olo Setup's `Step::SelinuxLabels`). As an ordinary program it ran
  as `unconfined_service_t`, which SELinux won't let start a session in
  the person's domain (`unconfined_t`). And the person's session starts
  through the system's own copy of olo-session (`olo-session --exec
  PROGRAM…`, which only hands over), as GDM's starts through
  gdm-wayland-session: SELinux moves into the person's domain as it
  starts a system program; `~/.local/bin/olo-session` (labelled
  `gconf_home_t`, as everything in `~/.local`) is no program the login
  screen's domain may start.
* **Only the ways in this computer has**: a fingerprint where Fedora's
  fingerprint stack is set up (fprintd's module in
  `/etc/pam.d/fingerprint-auth`; otherwise authselect leaves
  `pam_debug.so auth=authinfo_unavail` there, which says exactly that,
  and the greeter showed it), the face where Olo's face module is
  installed (`way_here`).
* **One greeter.** Its sessions are logind's, not the service's, so a
  restarted olo-login started a second beside the first, both drawing;
  starting, it ends what's left of its user's (`TerminateUser`).
* **It steps aside where it can't show.** Its greeter's session ending
  three times in a row before the greeter ever connected (no GPU Olo
  can render on, a broken install) hands the seat to the computer's
  other login screen for this boot (GDM, else SDDM, else the text
  console's login), notes why in `/run/olo-login/handed-over` (Olo
  Doctor says so) and exits 75, which its unit doesn't restart
  (`RestartPreventExitStatus=75`); the next boot tries again
  (`hand_over`).

**The boot splash** (app_server `plymouth.rs`): `olo-app-server
--plymouth-theme DIR [--scale S]` makes a theme for Plymouth's two-step
plugin from the start-up screen's own drawing (`veil.rs`): the wordmark
on Olo's dark, its four icons lit one by one as the boot goes, "Starting
Olo…" under them; the wordmark alone as the computer shuts down; a
disk's passphrase asked in the kit's focused text well (the login
screen's field) in the icon row's place, the kit's bullets in it. Two-step
stretches its pictures for device scale 2, so the theme is made at the
scale the computer's own screen is at in Olo (the built-in panel's,
else the main display's) and Plymouth draws device pixels
(`DeviceScale=1`): pixel for pixel on every screen, the start-up
screen's size on the computer's own. Change that screen's scale and run
`tools/install-login.sh --plymouth` again (it makes the start-up image
again, the splash being in it). Why each file is as it is (two-step's
centring, its frame choice, where it starts the bullets) is in the
module's comment.

**Testing.** Never with the real PAM, the machine's seat or its boot:

* `cargo test -p olo-login` (`a_login_from_start_to_finish`: the daemon
  with `--test DIR`, a greeter asking who and which sessions, a wrong
  password and the right one through a worker's PAM conversation on a
  stand-in stack, `DIR/pam/olo-login` as `pam_exec` of a script, the
  session started, run and ended); `cargo test -p olo-greet`.
* The greeter on a screen: `olo-login --daemon --test DIR` with
  `DIR/pam/olo-login` (a stand-in stack) and `DIR/root` (`etc/passwd`,
  `usr/share/wayland-sessions`, `var/lib/AccountsService/icons`), then
  `olo-greeter` in a tsession with `OLO_LOGIN_SOCKET=DIR/greeter.sock`;
  review it at every scale, light and dark.
* The boot splash: `cargo test -p app_server plymouth` (the frames match
  the watermark above the row, the field sits on the row to a pixel on
  screens of either parity, the first bullet where the kit's text
  starts, the measures the kit's); `OLO_PLYMOUTH_SHEET=DIR cargo test
  -p app_server plymouth_sheet` draws it as two-step would, at each
  scale; and `tools/dev/plymouth-check [--scale S] [--size WxH] [--keep
  DIR]` tries it in the real Plymouth: a start-up image of this kernel
  (dracut as you, the theme appended) booted in a VM of its own, its
  screens checked against the theme pixel for pixel, and a LUKS disk's
  passphrase typed through the VM's keyboard. (plymouthd on a vkms seat
  can't do: its renderers draw only with a virtual console of their own,
  which would switch your screen.)

## Testing: headless sessions

Every crate's tests at once: `cargo test --release --workspace --exclude
gpui-macros --no-fail-fast` (1,459 pass, 2026-11-07). Left out and known
to fail are vendored code's own tests needing features Olo doesn't build:
gpui-macros' `derive_inspector_reflection` (its inspector; it doesn't
compile, and would stop the whole run) and six of Smithay's doc examples
(its `renderer_test`, which its own CI turns on); Smithay's 82 unit
tests pass in the same run (not alone: `-p smithay` builds it without
the features the workspace gives it). Run it after a change that crosses
crates, and before saying a change is done.

Never open windows on the user's screen. `tools/dev/tsession` runs a
headless app_server (GPU, offscreen, 2560×1600 device pixels at 2× by
default, i.e. 1280×800 logical) and drives it:

```sh
T=tools/dev/tsession
$T start mine --light          # unique session name per developer/agent
$T start mine --session [--shell]   # under olo-session (with Deskbar + desktop)
$T start mine --session --home /tmp/olo-dev/mine-home   # sandbox HOME
$T reload mine                 # in-place reload (test upgrades: rebuild, then reload)
$T run mine olo-myapp       # binaries from target/debug (log in /tmp/olo-dev/mine/)
$T start two 2560x1600,1920x1080   # two screens, HEADLESS-1 and -2 (each offers a few modes)
$T shot mine /tmp/x.png        # full screenshot (device pixels) of the pointer's screen
$T shot two /tmp/y.png --output HEADLESS-2
$T shot mine /tmp/p.png X Y W H --pointer   # with the pointer drawn in
$T zoom mine /tmp/z.png X Y W H 4   # crop in logical px, magnified 4× (nearest)
$T click mine X Y [right]      # logical coordinates (screenshot px / the scale)
$T tap mine X Y                # press and release in one go, as a touchpad tap
$T drag mine X1 Y1 X2 Y2
$T type mine "hello"           # $T key mine 29 30  → Ctrl+A (evdev codes)
$T theme mine dark
$T start mine --scale 1.5      # another scale from the start
$T scale mine 1                # every screen to 1× (then 1.5, then 2: check all three)
$T hz mine 120                 # every screen at 120 Hz (60 to start)
$T stop mine
$T start mine --owner PID      # ends (stopped, folder removed) when process PID does
$T tidy                        # what ended sessions left in the runtime dir
```

`stop` ends everything of a session: its process group, its bus, what
`run` started, and anything carrying its environment (its display and
its own config dir: olo-termd and its shells, a program started and
forgotten; the person's session never matches). It waits until all of
it has gone, killing what hasn't after 3 s, so a tool that stops a
session and goes on finds nothing of it still running; then it removes
the state its apps left for their next selves (`olo-restore-PID.json`:
an app that saw app_server go writes it and re-executes, and killed
first, it stays) and the display's files in the runtime directory
(its socket and lock, the apps' sockets, layouts kept for a reload),
holding the display's lock as app_server takes it, so a live session's
are never touched. A tool that starts sessions passes `--owner` with
its own pid (the fuzzers, ui-lint, bench, order-check, rescue-check):
a watcher (`tsession _watch`, blocked on both processes' pidfds, no
polling) stops the session and removes its folder if the tool ends
first, killed or not, so a tool cut short leaves no app_server running
for a day.

Each headless screen draws at its mode's rate (60 or 120 Hz), on time:
check anything that moves at 120 Hz too. Every kit app's `{"frames":true}`
(on its `olo-menus-PID.sock`) answers with each frame it drew since the
last ask, the last 1200 (`FrameMark`): when it began (CLOCK_MONOTONIC
µs, app_server's clock), how long it took, its parts (`ui_us`, laying
out and painting: the app's code and GPUI's; `submit_us`, the rest:
recording, submitting and presenting), and where
each scroll view showed its content from and how far it scrolls
(`[x, y, max x, max y]`: past either end, it's pulled past its edge).

`tools/dev/scroll-check NAME --app NAME` measures scrolling with them: on
a long list under the pointer it drives a steady two-finger scroll (at a
touchpad's rate, `--rate 90`), a flick and its glide, and three wheel
notches, and prints for each how regularly frames came (hitches: a frame
later than 1.5× the usual while the content moved), how evenly it moved
(each frame's step against the fingers' speed; the glide's against its
neighbours, as it slows evenly; each notch easing out with no step
bigger than the one before; both only until the content reaches its end,
whose rubber band and bounce aren't the fingers'), frames that drew
without moving, how long the first move took to show and the slowest
frame; `--trace` prints every frame with its parts and whether the
content was at or past its end. Smooth is
every step within a few percent, no hitches and no still frames, at 60
and 120 Hz and every scale. Run it after changing anything about input,
frame timing or a scroll view.

Every session gets its own D-Bus session bus (`olo-bus` from the build
tree, as agent screens have, else busd; at `/tmp/olo-dev/NAME/bus`, in
`DBUS_SESSION_BUS_ADDRESS` for `run`): the runtime directory is the real
one, so without it programs fall back to `$XDG_RUNTIME_DIR/bus`, the
user's real session bus, and a test's notifications pop up on their
screen. tsession won't start without one (`TSESSION_BUSD` for a busd of
your choosing, or, with no `olo-bus` built, the busd it builds under
`~/.cache/olo-dev` with `tools/dev/busd-lenient` the first time on a new
machine). To see a
test's notifications, run its own centre:
`$T run NAME olo-notifications --bus unix:path=/tmp/olo-dev/NAME/bus`.

sudo resets HOME, so without `--home` a session and its apps use the real
home folder: pass `--home` whenever a test saves files (Screenshot,
Tracker, StyledEdit…). What apps keep between runs and their caches are
the session's own even so (`XDG_STATE_HOME`, `XDG_CACHE_HOME` in
`/tmp/olo-dev/NAME`, in its env.json for `run`): a test Terminal (the
fuzzers open them) once restored the person's windows and wrote its own
over their `windows.json`, which their next login would have brought
back. Sessions are always silent (`PULSE_SERVER` points at
a dead socket), so tests never reach the real speakers or volume. To
test sound, run `olo-fakesound SOCKET` (olo-preview): it speaks the
PulseAudio protocol, plays nothing, and logs each stream once a second
(bytes consumed in real time, corked or not, loudness); start the app with
`$T run NAME PULSE_SERVER=unix:SOCKET …`.

The headless server renders on the GPU's render node and announces it
in dmabuf feedback, so GL clients (Chrome) get hardware GL
as on the desktop rather than Mesa's software fallback.

The fuzzers are one program, `olo-fuzz` (crates/fuzz, built with the
workspace: `target/release/olo-fuzz`), in Rust like the rest. Every run
is in a sandbox of its own (`sandbox.rs`), and starts the session its
fuzzer wants there itself: the fuzzers press, drag and type into real
apps, and a drag that lands in a Tracker window instead of on a seam
moves the files it shows (one tried to move /usr/share/backgrounds), so
they never see the person's files. Run from outside, olo-fuzz makes new
user, mount, PID, network and IPC namespaces (no root) and runs itself
again inside, as the first process's child; inside are read-only
/usr, /etc, a sysfs of its own, the GPUs' device nodes and, of the
repository, only `target/release` (`target/debug`), `data` and `tools`;
its own /proc, ptys, /run and runtime directory, and a home, /tmp and
/var/tmp on disk (`/var/tmp/olo-fuzz-sandbox-PID`, removed after); no
network; a fresh environment (none of the person's session's
variables). The one folder shared with the machine is
`/var/tmp/olo-fuzz`, for results. When the run ends, the kernel ends
whatever is left in its namespace, and each process of the sandbox dies
with its parent: killed outright, a run leaves nothing running.
`olo-fuzz sandbox-probe` says what a program in it can see and do (its
test checks it's sealed). Each session there has a home of its own with
a few folders and files for Tracker to show.

Each fuzzer talks to app_server over one connection kept open
(`olo_protocol::Client`) and starts the session's programs itself, and
a sweep runs many seeds at once as threads of one process. Each takes
`--seed N` (a run is reproducible from its seed, printed first),
`--steps N`, `--only OPS` (just those steps), `--keep-going`,
`--trail STEPS` (every window's frame as it changed during those steps,
with when), `--keep` (a failure's session logs kept in
`/var/tmp/olo-fuzz/KIND-run/`) and `--debug` (debug builds: every
lock's order checked, olo_sync; slower).

`olo-fuzz wm [--tiles] [--trace-tiles]` is an
adversarial window-management fuzzer: on a session with two screens and
the shell (2560×1600 and 1920×1080) it opens
and closes Terminal, StyledEdit and Tracker windows and drives random
combinations of every arranging command, shortcut, switcher, workspace
and display change, checking after each step that app_server answers,
every shown window (on the workspace its display shows) is on a display
with its tab in reach, one shown window at most has the keyboard (none
only with several displays, the active one showing none), commands that
shouldn't move other
windows don't, and round trips (zoom/unzoom, tile/Restore, Undo, a
workspace or display and back) come back exactly. It ⌘-drags tabs along,
out of and into groups, and uses GroupWindow / UngroupWindow / MoveTab,
checking that groups hold together (one frame, tabs side by side in
order, other apps' windows only by hand). Now and then it opens a
Terminal into a Terminal group sized by hand with a button held down
(the pointer grabbed, as after a menu) and checks the group keeps its
size. It also has apps resize their
own windows (`op_app_resize`: a new Terminal tab, closing it, a bigger
or smaller text size, on a window as it is, zoomed, tiled or as tall as
its display) and checks that nothing grows past its display or out of
the size it was zoomed or tiled to; zoomed windows must stay inside the
tiling area after every step. Every tiled window on the active display
is checked against its tile's part (`WindowInfo::part`), reported once
each, failing with `--tiles` (a frame and tile that part ways jump when
the tiles are next laid out); `--trace-tiles` prints each window's part,
frame, workspace, group and hidden state whenever they change, to find
the step where something went wrong. After each command it waits until
no window is settling (`WindowInfo::settling`: its program still to draw
a size or state it was asked, a configure unanswered or answered and not
yet committed) and nothing has changed for a quarter-second, so a slow
client isn't taken for a wrong frame; a failure prints, under `seen`,
every window's frame as it changed during the step, with when.
`--only app_resize,tab_in` runs just those steps (wm.rs's `OPS`). Run
it after changing anything in shell/.

`olo-fuzz auto [--reloads]`
is its auto-tiling counterpart (on a session with the shell, one
3024×1890 screen at 1.75×):
it turns Auto-Tiling on for the first four workspaces and, mostly, drags
the seams between tiles the way people do (fast and slow, a few steps or
many, past where it stops and back, held still before letting go), with
windows opening, closing, grouping, ungrouping and changing tabs and
workspace trips between; `--reloads` restarts app_server now and then.
After each step, once the windows settle, every tile (one per group) must
be inside the tiling area with no two overlapping, and stay put for a
moment after (nothing still on its way to land somewhere else). During
each drag it samples the tiles every few milliseconds on a thread of
its own (`MidDrag`): no tile may be over another, and tiles side by side
as it began, the tile gap apart (as app_server links them), must stay
that gap apart in every look (`check_mid_drag`). The build before drag
rounds failed every drag of it, by up to 632 px. An edge is grabbed
below both windows' tabs (beside a tab is whatever is behind it, the
desktop: a press there took the keyboard from every window). Each
step's line has the time, to find it in the session's log.
It floats too: a tile floated or a float tiled (the window menu's
Float Over the Tiles), and windows brought forward, a float then being
the front one of the floats; after every step no floating window may be
under a tile (`check_floats_over`). Run it after changing anything in
`autotile.rs`, `tiling.rs`, `rounds.rs` or the resize grabs.

`olo-fuzz sweep [--seeds A-B,C] [--jobs N] [--steps N] [--confirm] [--keep] [--debug] -- wm|auto|mode [ARGS…]`
runs a fuzzer over many seeds at once, in one sandbox, each in a
session of its own that it starts (the right screens for the fuzzer)
and stops, each
fuzzer a thread, the sessions started a quarter second apart. How many
at once is set by memory, not the processors (the fuzzers mostly wait
for windows to settle): a session costs about 450 MB at its peak
(24 at once took 11 GB, the processors still 80 % idle), so it takes
600 MB a session (wm 365, mode 462, auto 450 at their peaks) out of
what's free less the person's reserve (a quarter of the memory, 8 GB at
least). The machine is shared, and what's free moves as a sweep runs:
a session starts only while there's room for it above the reserve, the
sweep stops (as Ctrl-C would) if what's free falls under half of it,
and everything in the sandbox is the first to go if memory runs out
(`oom_score_adj` 1000). A wave sized once at its start ran the machine
out and the kernel killed Ben's Slack. By default a
sweep is one wave of that many seeds, about two minutes for 120 steps
of auto (one at a time, an hour). `--confirm` runs the failures again
by themselves (all at once: a few sessions are next to no load),
telling the load's (a timing check missed under many sessions) from a
fault. Each seed's output is in `/var/tmp/olo-fuzz/sweep-SEED.txt`,
and `--keep` keeps a failure's session folder (its logs) as
`/var/tmp/olo-fuzz/sweep-SEED/`.

A run leaves nothing behind: each session is stopped and its folders
removed however its seed ends (passed, failed, the fuzzer panicking,
Ctrl-C: the fuzzers stop after their step, a second Ctrl-C ends it at
once); the programs it started are waited for as they quit, and the
sandbox's first process waits for every other (no zombies); and
whatever is left goes with the sandbox, killed or not. Sweeps in
sandboxes of their own can run side by side.

`olo-fuzz mode`
drives the windowing modes (on a session with the shell and two
displays, 1920×1200 and 1440×900): each
display's mode changed at random among
Standard, Auto-Tiling and Focus, with windows activated as Deskbar does,
brought forward by launching their app again (Tracker on a folder it
shows), ⌘Tab, hidden and shown, sent to the other display, workspace
trips, and windows opening and closing. After every step it checks each
mode's rules: on a display in Focus exactly one unit (a window that
would be a tile, with its group) shows, its front zoomed, the rest
covered, and the window with the keyboard is the one showing; elsewhere
nothing is covered; a window brought forward has the keyboard, shows and
leads its group; and a Focus round trip (Standard, Focus with
activations, Standard) puts every window back in its own frame.
`olo-fuzz mode --matrix [--chrome] [--only WORDS]` is its deterministic
half: in every mode a Tracker window behind the Terminal with the
keyboard, hidden, on the other display and on another workspace is
brought forward each way (Deskbar's, no token; launched again, with the
token it asks app_server for; launched again with a token in its
environment that isn't for it, as a link's opener leaves one), and must
have the keyboard, show, be in front on its display
(`WindowInfo::stack`) and move nothing else; it prints the table.
`--chrome` adds Olo's Chrome (not one of Olo's programs, so it comes
forward only for a token app_server takes: the case a link opened from
Terminal is), on a profile of its own with no network, from a copy of
the installed Chrome in the session's home (the sandbox lends it the
installed version's folder, read-only, and nothing else of it: not the
extension's signing key); its first window takes most of a minute. Its
Chrome is stopped and its folders removed however the matrix ends. Run both after changing `modes.rs`, groups, activation
(`request_activation`) or how a launcher gets its token: put back the
token olo-chrome passed before its fix and the matrix fails every
Chrome case with the opener's token.

`tools/dev/upgrade-check NAME [--times N] [--reload-only] [--no-touch] [--auto-tiled]`
checks that windows come back from an upgrade exactly (display,
workspace, frame, minimized); `--auto-tiled` arranges three windows in
Auto-Tiling first, the case that broke when Olo's GPUI first committed a
window before naming it (it sets the app id and title before the first
commit now: app_server claims a returning window's place, its tile, at
that commit). Arrange a `--session --shell` tsession
(auto-tiling with edges dragged, floating and zoomed windows, other
workspaces and displays showing, a minimized tile, an outside program in
a tile, a window hanging over onto another display), then each round it
snapshots every window (showing each workspace of each display in turn),
upgrades as olo-session does (every Olo binary touched, every Olo
program of that session sent SIGURG to re-execute, app_server restarted
at once, so windows close and return while it goes) and compares. Run it
after changing `layout.rs`, `autotile.rs`, the kit's reconnecting or
anything an upgrade goes through, at 1×, 1.5× and 2×, and with
`OLO_HEADLESS_LATE_MS=600` in the environment the session starts with:
the headless displays then come that long after app_server starts, one
after another, as DRM's do on real hardware, after the clients have
reconnected (a headless session otherwise has its displays at once, and
can't show what follows).

What follows, on a real computer: windows come back before their
displays. A returning window whose display (its `Placement::screen`)
isn't found yet waits out of sight, its place kept, until it is
(`AnvilState::waiting_for_display`, released by `displays_changed`,
which the backends call as displays come and go) or `DISPLAY_WAIT`
(8 s) is over, when it is fitted onto another as before; meanwhile the
auto-tiling trees of displays not found yet are kept, not dropped as a
gone display's are (they were, and a display's tiles were then made
afresh as its windows came back, in whatever order they came: windows
swapped tiles). `CHECK_AFTER` (3 s) later, every window put back is
compared with what was saved (display, workspace, place, minimized):
any that ended up elsewhere is put back as saved (unless auto-tiled, or
moved since on purpose: by hand, a shortcut or an agent,
`WindowMeta::moved_on_purpose`), and each difference is logged
("came back from the restart other than it was"), with a summary
("windows back after the restart, checked against what was saved").

A window goes back to the same place on the display it was saved on
(`Placement::screen_key`, the monitor, else `screen`'s connector),
wherever that display is now (`saved_location`): the displays found one
by one are arranged anew as each comes, and a window put back by its
desktop coordinates meanwhile landed on another display. And a display
is known by its monitor before anything is done with it: one that can't
say yet (no EDID: Apple's DCP has it about a second after a new
app_server takes the displays over, as on an upgrade, so all of them
came up "Unknown", their saved places, scales and workspaces weren't
found, and the windows scattered) isn't set up until it can, for up to
`udev::IDENTIFY_WAIT` (3 s; returning windows wait for it). One set up
without (the wait over) is looked at for its EDID for a minute, and
when it comes, `display_identified` makes its settings, place and
workspace its monitor's, its windows going with it.
A monitor that takes its picture in tiles (a 5K or 6K display over two
DisplayPort streams: Apple's Studio Display, LG's UltraFine 5K) is a
connector per tile, each with the kernel's `TILE` property; most offer
the whole picture through one of them too. That one is the display and
the others are left off (`tiles.rs`, `BackendData::covered`): set up
one by one, the Studio Display was two displays, 5120 × 2880 and a
2560 × 2880 copy of its right half. When the whole one leaves, the
others are displays of their own again. A tiled monitor offering no
whole picture is still a display per tile.

**Colour: wide gamut and HDR.** Each display's colours come from its
EDID (`olo_system::edid::color`: the base block's chromaticities,
CTA-861's colorimetry and HDR static metadata, DisplayID 2's display
parameters, where a laptop's panel keeps its native primaries and
luminance) as protocol's `DisplayColor`, kept with its `Output`
(app_server's `color.rs`). How it's drawn (Preferences ▸ Display ▸
Colour, kept per monitor in its `OutputConfig`):

- *Plain*: an sRGB display, or a wide one set to Vivid. Nothing
  converted, clients' buffers straight to planes as ever.
- *Accurate* (a wide display's default): sRGB's colours converted into
  the display's gamut as they're drawn, so sRGB's red is sRGB's red on a
  P3 panel. Olo's Smithay renderer does it (`gles::color`): a frame
  draws into a `ColorTarget`, a texture holds a `ColorSource` (sRGB
  unless it says), and when they differ the default texture program's
  `COLOR` variant decodes (gamma 2.2: what sRGB displays do), converts
  between primaries in light (white adapted by Bradford, so greys stay
  grey), clips and encodes; solid colours are converted on the CPU.
  Clients' buffers don't go straight to planes then (they'd show
  unconverted); the pointer still may.
- *HDR*, on a display that takes it (its EDID says PQ, or a built-in
  panel gives an HDR luminance and its connector takes the metadata:
  the ThinkPad P1 Gen 9's OLED, whose HDR is in DisplayID, not its
  EDID's HDR block): the frame is composed into a float buffer in
  BT.2020 at gamma 2.2 relative to SDR's white (so blending and text look exactly as in
  SDR, and highlights go above 1), then what changed is encoded into PQ
  (`color::HdrFrame`, `HdrElement`); the connector gets HDR metadata
  (the display's own primaries and luminance) in a modeset commit of
  its own (`set_connector_color`, again at vblank if a flip was in
  flight), with the display off: the next frame enables it whole, as
  after idle sleep. The signal's primaries (`color::signal_primaries`):
  a monitor whose EDID says PQ takes BT.2020 and is told so
  (colorimetry `BT2020_RGB`); a built-in panel takes the signal's RGB
  as its own primaries whatever it's told, so it's sent in the panel's
  own (DisplayID's) with colorimetry Default (BT.2020's numbers there
  looked washed out: tried on the machine, 2026-11-05).
  The ThinkPad's OLED (Samsung ATNA60KA02, eDP 1.5) decodes PQ itself
  from the signal and its metadata. The kernel needs
  `enable_dpcd_backlight=3` to offer the panel's HDR at all (the VBT
  doesn't ask, the EDID has no HDR metadata), and with it Intel's
  driver uses Intel's HDR interface (DPCD 0x340 on) only for eDP 1.4b
  and older (`intel_dp_aux_init_backlight_funcs`): this panel gets
  VESA's luminance control instead (0x721 = 0x82, the target in
  millinits at 0x734; `intel_backlight`'s max is 656, in nits). In HDR
  the panel ignores that target (the backlight does nothing: PQ says
  how bright), ignores 0x344 written while it's on, and resets it as it
  powers on (tried by hand through the AUX channel, 2026-11-05). So
  app_server offers that panel's HDR only once the kernel started so
  (`color::kernel_drives_panel_hdr`); until then Preferences says it
  waits on a setting at start, and Olo Doctor offers it
  (`AddKernelArgument`). SDR's white is "Everything else's brightness"
  (80 to 600 cd/m², `color::MIN_SDR_NITS`/`MAX_SDR_NITS` in protocol,
  shared by app_server and Preferences' slider); unset, a built-in
  panel's follows its brightness (the panel's full-screen luminance
  times the level), so SDR is as bright in HDR as out of it, and other
  displays' is 203 cd/m², BT.2408's. Changing it only re-encodes the
  whole frame (`Backend::sdr_white_changed`: no modeset, no new
  buffers; the whole `color_changed` per step of a dragged slider
  blanked the OLED), and the brightness keys step it on a display in
  HDR (a log scale, 16 steps from end to end; `power::step_sdr_white`).
  Turned on in Preferences, HDR is kept only if the person says so: an
  alert asks, counting down, and turns it off again unless Keep is
  pressed in 15 s (`widgets::open_timed_alert`), as a new resolution is.

*HDR and wide-gamut video* (`olo_images::hdr`): a decoded picture knows
its primaries and transfer (H.273's codes: AV1's sequence header; else
its container's, MP4's `colr` and Matroska's Colour, through
`Frame::with_colour`, which also takes how bright it was mastered,
MaxCLL or the mastering display's peak), and `Frame::to_rgba` converts
one in BT.2020 or Display P3, or with PQ or HLG, for an sRGB screen: its
signal to light by tables (PQ absolute, counted in SDR's white, 203
cd/m²; HLG through BT.2100's display mapping for a 1,000 cd/m² screen),
BT.2020 to sRGB in light, highlights above four fifths of white rolled
off by the brightest channel (hue kept) to reach white at the mastered
peak (1,000 cd/m² unsaid), what's outside sRGB clipped, sRGB's curve.
Every step per sample is a table or a few multiplies, on up to eight
threads for a megapixel or more: a 1080p picture 4.6 ms, 4K 14.6 ms
(SDR's plain path, one thread: 5.2 and 16.8). A wide video's pictures
(BT.2020 or P3, HDR's among them) are converted into Display P3
rather than sRGB (`Frame::to_rgba_wide`, tagged so: `Picture::p3`,
`RenderImage::with_gamut`), and Viewer's window draws in P3 while one
shows (`PreviewEvent::Gamut` from its first picture), as for a wide
photo: on a P3 panel its colours aren't clipped to sRGB's. Viewer's
facts say it ("Colour: HDR (PQ), BT.2020, mastered to 1,000 cd/m²").
On a display whose HDR is on, Viewer draws an HDR video's window in
HDR10 (GPUI's, below) and the player makes its pictures HDR10's
(`Video::draw_hdr`: `Frame::to_pq_half`, half floats of PQ in BT.2020,
the stream's own PQ as it is, HLG and SDR converted): its highlights
shine as mastered, its paper white at SDR's; where the window can't, P3
or sRGB as above. (The preview tells its window when its video turns
out HDR, `PreviewEvent::Gamut`, whichever picture showed first.) HEVC's
colour is also read from its stream (the SPS's VUI, `hevc::colour_of`,
through the vendored rust_h265's `stream_colour`), where the container
says nothing: many files say HDR10 only there. `tools/dev/hdr-check
--clip VIDEO` plays one in Viewer on the pretend HDR screen. Chromium's
`bear-1280x720-hevc-10bit-hdr10.mp4` (its test data: x265's HDR10 tags
on its SDR bear, so it blows out, as tagged) reads as HDR and reaches
the display's 1,600 cd/m² there.

The display hardware's own colour stages (the CRTC's degamma, CTM and
gamma LUTs; Intel's per-plane pipelines, `COLOR_PIPELINE`) aren't used:
their lookup tables are uniformly spaced, 129 and 1024 entries, and
darks lose 10 of the first levels re-encoding light (PQ, worse). The
shader's cost, a whole 3200 × 2000 frame on the ThinkPad's GPU: 0.618 ms
plain, 0.705 into P3, 1.53 for HDR's two passes (`color::tests::
gpu_cost`); frames draw only what changed. `color::tests::
the_gpu_converts_as_the_cpu_does` reads the GPU's conversions back
against the CPU's. A headless screen pretends to be wide or HDR with
`OLO_HEADLESS_COLOR=p3` or `p3-hdr`.
`tools/dev/hdr-check` tries it all end to end on such a screen: a window
tagged PQ and one untagged (`color_probe`), the screen's own buffer read
back (`{"type":"screenshot","as_shown":true}`, headless: what a display
would be sent, not the sRGB picture a screenshot is) and checked against
what's computed: greys kept and sRGB's blue as P3's numbers in Accurate
SDR; in HDR, sRGB's grey at its light against SDR's 203 cd/m², PQ's 100
cd/m² as it is, nothing above SDR's white.
On the machine, `tools/dev/panel-hdr-check` checks a built-in panel's
HDR as the panel itself has it (in the person's session, with their
agreement; sudo for the AUX channel): its HDR registers (DPCD
0x340–0x35F, each bit named) in SDR, with HDR on through app_server and
off again, and whether the panel showed a picture each time.
`cargo run --release -p olo-ui --example hdr_card` opens a test card
drawn in HDR10 for looking at a panel by eye: SDR's white beside 400,
1,000 and 1,600 cd/m², and 0.1 to 10 cd/m² below (decoded, the top
ones far brighter than the white and the first two below next to
black; undecoded, PQ taken as gamma, all of them washed out).

Programs say what colours their windows hold with `wp_color_management_v1`
(`color_management.rs`, version 1): parametric image descriptions only
(gamma 2.2, sRGB, linear and PQ; sRGB, BT.2020, DCI-P3, Display P3 and
Adobe RGB, or any primaries; luminances; mastering metadata taken and
not used). A surface's description is cached state (Smithay's
`SurfaceColor`), applied with its next commit, and the renderer converts
its buffers from it as they're drawn (`Frame::set_texture_color`), never
straight to a plane; a display's description says how app_server draws
for it (PQ in BT.2020 with SDR's white as the reference, its own
primaries and light as the target, in HDR; else its primaries at gamma
2.2), and programs are told when it changes (`image_description_changed`,
`preferred_changed`). `cargo run --release -p app_server --example
color_probe` (in a test session) prints what's offered and shows a window
tagged PQ: PQ's 100 cd/m² shows as sRGB's 185 where untagged it's 130.

Pictures carry their colours too: `olo_images::open` converts one from
the ICC profile its file embeds into sRGB (`to_srgb`, moxcms, perceptual),
so an iPhone's Display P3 photo shows its colours, not P3's numbers taken
for sRGB's (washed out); 34 ms for 12 megapixels, on the thread that
decodes it. Olo's apps draw in sRGB, but a window can draw in Display
P3 (GPUI's `Window::set_gamut`): its swapchain made again in Vulkan's
`DISPLAY_P3_NONLINEAR_EXT` (ten bits a channel where offered; Mesa's WSI
tells app_server, which converts the window into the display's colours),
its sRGB colours converted into P3's numbers by the shaders (gamma 2.2
both ways: read back, a P3 window's grey is an sRGB window's, exactly),
and an image marked P3 (`RenderImage::with_gamut`, a kit `Rgba`'s
`gamut()`) drawn as it is; in an sRGB window a P3 image is converted.
Viewer draws in P3 while it shows a photo wider than sRGB
(`olo_images::open_wide` keeps it in P3), so an iPhone photo's reds and
greens show whole on a P3 panel. A window can draw in HDR10 too
(`Gamut::Hdr10`: Vulkan's `HDR10_ST2084_EXT`, ten bits, which Mesa's
WSI offers on app_server's surfaces): every colour encoded in PQ in
BT.2020 by the shaders (sRGB's white at SDR's 203 cd/m²), a P3 image
converted, and an HDR10 picture drawn as it is: `RenderImage::deep`,
half floats a channel in an atlas of their own (`AtlasTextureKind::
Deep`, `Rgba16Float`: eight bits of PQ would band). In another window an
HDR10 picture is shown to SDR's white. The kit tells GPUI which
displays' HDR is on (`gpui::set_display_hdr`, from app_server's
`OutputInfo::hdr`), and `Window::on_hdr_display` says whether a window
is on one (once it has entered its display, after its first frame).
`tools/dev/hdr-check` reads a GPUI window in HDR10 back from a pretend
HDR screen (the kit's `hdr_probe` example): its sRGB white at 203
cd/m², its HDR10 pictures at their own 1,000 and 100. A window not
asking costs nothing (one uniform: bench's frames the same before and
after, StyledEdit and Viewer at 1×). GPUI never makes colour management
objects itself: Mesa's WSI makes the one a surface may have. Night light in HDR scales each channel's light
and encodes PQ again (`night::ramps_pq`), the gamma ramp's input being
PQ's code (evenly spaced to the eye: the LUT holds it well).
`tools/dev/layout-check NAME [--auto] [--unplug]` checks this quickly:
it arranges a session's screens in an order unlike the one they're
found in, puts windows on each and on another workspace (auto-tiled with
`--auto`), reloads app_server and compares every window's display,
place, size, workspace, and each display's place and workspace. With
`--unplug` it unplugs the first screen instead (`plug_display`; with
`--auto`, holding tiles of thirds), checks its windows went to the main
display on the same workspaces, plugs it back and compares. Run it with
`OLO_HEADLESS_LATE_MS=700` and with `OLO_HEADLESS_ANONYMOUS_MS=1500`
(the screens say who they are only that long after they're found) in
the environment the session starts with.

`tools/dev/perf-fuzz NAME [--seed N] [--steps N] [--keep-going]` is its
counterpart for speed: on the same kind of session it hammers the
desktop with random bursts (dragging and resizing windows, Cmd+Tab and
workspace spam, arranging, opening and closing many windows, flipping
the theme and a display's scale, flooding a terminal, scrolling, typing,
keyboard hints) and checks each one against budgets: app_server's event
loop never stalls for more than two frames (`{"type":"timings"}`: a
heartbeat that runs while anyone asks, whose lateness is whatever held
the loop, with the slowest request, input event, clients' requests or
tick named; and a ping every few milliseconds all through the burst), no
app_server frame takes longer than a 60 Hz frame to draw, every kit app
draws each frame within 50 ms, on the GPU, and answers within 250 ms
(`{"frames":true}`), and no file descriptors, threads or memory leak. It
ends with the worst numbers per kind of burst. Run it after changing
anything that runs per frame or per event. For the leak check it closes
every window that wasn't there when it started (apps open some of their
own), ends the program of one that won't close (a document typed into,
asking to save), and counts threads by name: a worker app_server starts
the first time it's needed (`tabs`, Smithay's shm pool dropping) is
named, not failed, and a failure says which descriptors grew (dmabufs,
sockets). Clients' requests that held the loop are named by the kind
of part that took longest in all (`Timings::within`, summed by kind: "a
new window ×5"), so a burst of small ones is found too. Note that on a
shared machine a stall whose slowest named thing is small is the
scheduler's (other programs had the CPU), not app_server's.

app_server's own text (tab titles, the switcher, hints, captions) has a
font system per thread that draws or measures (`text.rs`: made from one
database of the fonts found once, the tab family's faces mapped and
shared), so no thread waits on another's: the main loop measuring new
tabs' titles used to wait on the tab worker drawing them, behind one
locked cosmic-text (opening five windows at once: the loop held 7, 10
and 12 ms in three runs, two waits of 2.3 and 2.7 ms on that lock in
`strace`; per thread, 4.5, 5.0 and 5.5 ms, what's left being the
frame that first draws them, and app_server's memory the same).

`tools/dev/bench` is the speed benchmark, every app against the 120 Hz
budget (rule 9). On release builds (`cargo build --release --workspace`,
then `cargo build --release -p olo-ui --examples` for the kit's gallery,
apart: in one go cargo builds the apps with GPUI's test support, and
those mark no frames; `TSESSION_PROFILE=release` makes tsession run
them) it
starts a session of its own ("bench": a 3456×2234 screen, a 16" MacBook
Pro's, at 120 Hz, with the shell) and a sandbox home under
/var/tmp/olo-bench filled with what makes apps work (a 20,000-line
document, a folder of 3,000 files, photos, 200 notifications, a
terminal's worth of output), and at 1×, 1.5× and 2× runs each app
(the kit's gallery, Tracker twice, StyledEdit, Terminal, Viewer,
Notifications, Activity Monitor, DeskCalc, DiskUsage, Logs,
Preferences, the palette, the emoji picker (shown, typed in, walked,
wheeled, hidden and shown again: a resident picker's usual open),
Passwords, Screenshot, Telegram with a pretend
account of 3,000 chats) through what people
do: launched and zoomed to fill the screen, left to load, idle for a
second (it should draw nothing), the pointer swept over it, each scroll
view scrolled (fingers, a flick and its glide, wheel notches, judged as
scroll-check does), its tabs clicked, its menus opened and walked, a
field typed in, the window resized live, the theme flipped; then
closed (and ended if it's still running, as a document typed into asks
whether to save, so the next scale's launch opens a window of its own
rather than handing its file to this one); then Deskbar and every
screen saver module. Every frame each draws comes
from its `{"frames":true}` marks (menus' too), app_server's from
`{"type":"timings"}`. A step fails on any frame over the budget
(`--budget`, a frame at `--hz`; the slowest frame's parts are printed
with it, so the fix starts in the right place), a skipped frame while
scrolling, or app_server over budget or stalled; switching the theme or scale, an
app's first frames and the start of a live resize (a new swapchain)
are transitions, listed with their slowest frame. `--apps a,b`,
`--scales 2`, `--json FILE`, `--list`. It only looks at Preferences
(its sidebar and tabs), never presses Return in a menu, and leaves out
what needs the real system or other sessions. Run it after changing
the kit, or anything an app does per frame, and before calling
anything fast.

Clicks are global logical coordinates: on a screen that isn't at (0, 0),
add its position (`$T ipc NAME '{"type":"outputs"}'`). `start` rewrites
the session's app_server.json, so test persistence with `--session` and
`reload`.

Look at your screenshots. Then zoom in to 4× and check edges, insets,
alignment and text placement pixel by pixel, in **both** light and dark,
and at **each of 1×, 1.5× and 2×** (`$T scale NAME 1`, `1.5`, `2`; a
fractional scale is where a half-pixel line blurs, a rounded inset
drifts or text sits a pixel off its neighbour). Every visual check covers
all three; none is done at 2× alone.
Compare against BeOS R5 / Haiku screenshots from memory. Fix anything that
looks even slightly off before calling something done.

### Layout review (`olo_ui::review`)

The review reads one frame's painted
shapes (text with its baseline, filled bands, lines, icons) at the scale
it was drawn at and flags what tends to look off: what you should look
at closely before calling a view done.

The checks are `olo_kit_base::layout_review`'s, on what the kit drew
(`Parts`): asked for a review, an app draws each window once more
(`Window::draw_now`, even one out of sight) while its `style`
primitives, labels, text fields and icons record what they drew, and
the `Frame`'s last element hands that to the checks. Nothing is
recorded otherwise.

- **Errors** (tests fail on them):
  - `blurry-icon`: an icon off device pixels, not a whole number of them,
    or a pixel or two off its texture's own size (`Marks::textures`), so
    it is resampled.
  - `line-weights`: one-point lines of different weights.
  - `blurry-outline`: a stroke not a whole number of device pixels.
  - `seam`: a line and what it borders a device pixel apart.
  - `off-pixel`: a fill with an edge between device pixels: snapped,
    it comes out a pixel off what it lines up with (rows of two heights,
    or a bar a pixel off the band beside it), and not snapped it blurs. A line
    is judged only across its thickness, and edges a clip cuts or on the
    window's own edges are skipped. The check runs only at scales where
    the 2-point grid lands on pixels (1×, 1.5×, 2× and up), not at 1.25×
    or 1.75×, where edges are still crisp but spacing may differ by a
    pixel.
  - Layout and spacing, on what the kit drew and the window's agent tree
    (every control as laid out, the clip it was painted in):
    `collapsed` (a control laid out to nothing), `outside` and `cut` (a
    control past the window's edge, or cut off by what it's in; what a
    scroll view cuts is scrolled out of sight, not cut: ScrollView,
    ColumnListView, their bars and a list's header mark themselves,
    `review::scroll_area`; a clip is the scroll view's only when it is
    the viewport, or the viewport with its frame and bar, not any clip
    with a scroll view somewhere in it), `overlap` (two controls over each other,
    neither inside the other), `text-outside` and `text-cut` (text past
    the window, or cut off: give it room or `style::fit` it),
    `text-over-text`, `overflow` (anything that starts in a box or a tab
    view's pane and runs past its inset, bottom or right: a tab's
    content under the panel's Defaults, a long check box into its box's
    frame; boxes and panes mark themselves, `review::container`,
    `review::pane`), `crowded` (two things, side by side or one over the
    other, closer than `metrics::TIGHT`; a list's rows and header, tabs,
    and what's on a grid of its own are spaced by the kit),
    `window-edge` (content nearer the window's edge than its inset, less
    4, unless in a bar or a view along that edge), `crowded-lines` (two
    long lines a point or three apart: a box's edge almost on its
    container's; a scroll view's frame and bars are the kit's),
    `control-edges` (controls down a column almost lined up, left or
    right, in the same section: check boxes' right edges are their
    labels'), `uneven-gaps` (a row of controls spaced unevenly by a
    point or few), `after-commit` (a commit row, push buttons Apply,
    Revert, Save or Done, not a toolbar's icons, with anything under it
    closer than `metrics::SECTION`,
    two spacings: the row ends its group, so what follows is a new
    section; as close as the rows above, the row reads as the start of
    what follows, as Preferences ▸ Power ▸ Graphics' Screen Recordings did).
- **Review** (most are mistakes, a few are meant): `near-height` (bands
  stacked or side by side almost one height: a 21 bar over a 22 head),
  `near-level` (text or icons in neighbouring columns a pixel or two off
  level, in one box: two boxes side by side lay their rows out each by
  itself), `near-edge` (left edges almost lined up down a column, in
  one box: what a box holds lines up with its own label and inset, not
  with what's outside it; not a column set from its right, whose left
  edges are its lengths'),
  `off-centre` (text or an icon off-centre in the band it sits in; only
  bands no clip cuts, since a card at the foot of a scroll view is
  centred in the whole of it, not in what shows), `uneven-rows` (rows
  down a column spaced unevenly). Right edges count as lined up a
  device pixel apart: text set from its right starts on a pixel.

A view tells it two things it can't see. `review::own_grid(window,
bounds)`: text laid out on a grid of its own (Terminal's screen, a cell
apart; a card in Notifications' history masonry, staggered in its
column), not held level with what is beside it, nor in a row with
what is above it. A scrolled `ScrollView` marks its content so itself:
a list's heading is a row apart from its first row only at whole-row
offsets. `review::band(window, bounds)`: what is in `bounds` is centred
across it though nothing fills it (a `TabStrip` tab, its top below the
strip's), so centring is measured against it. Call either as it paints.

Run it:

- on a running app: every app answers `{"review":true}` on its
  socket with a `commands::WindowReview` per window (`commands::review(
  pid)`): `$T review NAME [--all]` on a headless session, `olo-screen
  review NAME [--window ID] [--json]` on an agent screen. Switch the
  scale (`$T scale NAME 1`, `1.5`, `2`) and theme and review each.
- headlessly, as a test: the layout lint (below).

### Windows fit what they show (`WindowOptions::fits_content`)

A window whose content sets its size (a form, a Preferences panel, a
dialog, an alert) follows it, on every screen. `fits_content()` (every
`alert()` and `dialog()` has it; Preferences' panels, Drive Setup's
forms and the network sheet ask for it) lays the content out in a flex
item based at the window's designed height (`size`), its minimum what
the content needs, measures it (`Frame::follow`) and resizes the window
to it: a `fixed` window as tall as its content, `size` at least,
growing and shrinking with it (a tab with more rows, a display's
brightness, a longer message); one the person or the app sizes (a file
panel, Quick Look sized to its picture) never shorter than its content
needs. Never taller than the shortest display less 64 points
(`screen_room`, from the displays GPUI knows, in points, as turned; a
display coming or going looks again): past that the Frame lays the
content out at its own height and scrolls it, the window widened by the
scroll bar. Layer surfaces, lock screens and full screen windows are
the screen's or the app's to size.

It opens at that size: nobody sees it at its designed size first and
then grow. The Frame measures the content as each frame is painted; a
frame drawn at a size about to change isn't shown
(`Window::skip_present`, GPUI: nothing is committed, and the resize
draws the next frame, at its size), and no frame is shown until the
size asked for is made (GPUI's Wayland window sets its geometry, and a
fixed window's least and most sizes, as its size changes). What the
window asks for as it opens, from a worker, would still grow it a
moment later: hold its first showing until that's in with
`olo_ui::hold_first_show(window)`, a hold that's `Send` (give it to the
worker) and let go by dropping it, after the answer is where the view
reads it. The window is shown when every hold is let go, or after
`FIRST_SHOW_WAIT` (150 ms) whatever comes, so a slow answer never keeps
it hidden (it grows when the answer comes, as before). Preferences
holds every panel for its settings and app_server's displays, Display
for the GPUs, Date & Time for timedated's status. Don't hold for
anything slow or endless (a list streamed in, a scan): show those as
they come.

So: never wrap a whole window's content in a scroll view of its own
(the lint can't see past it: Display hid an 804-point panel in a
658-point window, its scroll bar against the boxes and its last rows cut
off, at 200 % on a 3200 × 2000 laptop); never compute a window's height
by hand (`window.resize` with summed rows); a list shrinks and scrolls
(`flex_1().min_h_0()` round a `ScrollView`), nothing else does. Tab
views in such a window take `TabView::grows()`, so the tab view is as
tall as the chosen tab's content. What doesn't fit 1280 × 800 points
(the smallest screen Olo lays out for, a 13" laptop at 2×) is laid out
again: side by side (Display's settings beside its panel's facts,
Performance's thresholds two by two), in tabs (Performance: Supervisor,
Watchdog), not scrolled.

### The layout lint (`olo_ui::lint`, `cargo ui-lint`)

A window's view drawn as it will be shown, without a session: GPUI's
test platform with Olo's text system and fonts
(`TestAppContext::with_olo_text`), the window opened as an app opens it
(`open_window`, the kit's `Frame` around the view), at its size, on a
1280 × 800-point display (GPUI's test display: the smallest screen Olo
lays out for, so a window that follows what it shows is checked against
it), a window that resized itself given that size and drawn again
(`settle`, as a compositor would) and judged at the size it ends at, in
light and dark, at 1×, 1.5× and 2× (`VisualTestContext::set_scale_factor`,
the scale changed on the open window, as a display's change does). Each
time it is reviewed (the errors above), every tab of its tab views too,
and the lint's `fit` check runs over all of them: the window fits what
it shows, nothing running out of its boxes or pane (it says how much
taller: "make it 742 tall"), its tallest page ending the window's inset
above the bottom of its pane (or of the window), with no more than
twice that empty (`Lint::keeps_room(points, why)` allows room for a state
the lint doesn't draw: Telegram's QR sign-in, a finger being enrolled).
A control never shrinks below its height in a squeezed column, and a
Preferences panel's body never below its content, so too little room
shows as overflow the check can measure. `Lint::window(options, build)`
returns what it found; `assert_clean` fails a test with it, a line each.
`Lint::trees(options, build)` gives what agents read of the window
instead (`agent::tree`, 1× light), as it opens and with each of its
tabs clicked: for indexes made from what windows show (Preferences'
settings).

**Every app, live** (`tools/dev/ui-lint [APP…] [--review] [--scales …]
[--screen WxH] [--jobs N] [--no-cache]`): every app at once, each in a
headless session of its own with a copy of the made-up home and its own
sound server, three quarters of the processors' worth at a time (20 s
for all of them on the ThinkPad, from 5 min 43 s one by one); the look
and scale are set in one request and the app asked again until it has
drawn them (`WindowReview::scale` and `dark`), never slept for. An app
whose build and lint are as when it last came out clean isn't run
again (`~/.cache/olo-dev/ui-lint.json`: its binary's, app_server's and
the fake sound server's size, time and inode, the lint's scripts, the
screen, scales and --review); one with errors always is. Each Olo app
with a window opened in a headless session of its own (a MacBook's screen, 3024 × 1964 at 2×, or `--screen`'s:
run it at the screen you're on too, and at 2560 × 1600, 1280 × 800
points), a made-up home from `fixture-home`, its own D-Bus,
demo data where the app has it), every window and every tab of its tab
views reviewed at 1, 1.5 and 2×, light and dark, through its socket:
the same checks on the real app. Errors fail it; `--review` lists the
review findings too. Release builds (`cargo build --release
--workspace` first). Preferences' panels run one by one
(`prefs:touchpad`).

**Formatting**: the whole workspace is rustfmt's, each file by the
`rustfmt.toml` nearest it: Olo's own at the root (lines up to 200 wide,
`use_small_heuristics = "Max"`: a builder chain or a match arm on one
line), upstream's in gpui/ (rustfmt's defaults; Blade's crates edition
2021) and smithay/ (110 wide), so merges from them stay small.
`cargo fmt` (not `--all`: that formats path dependencies too, vendor/'s,
which stay as their authors wrote them) formats everything;
`cargo fmt --check` says whether it is.

**With every commit and push** (`tools/hooks`, installed by
`tools/setup.sh`, or `tools/hooks/install`): the pre-commit hook formats
the Rust staged (rustfmt, as above; vendor/ left alone) and stages it
again, and, when UI code is staged, runs `cargo ui-lint`;
the pre-push hook runs `tools/dev/ui-lint` on the apps the push changed
(all of them when the kit or GPUI did). A finding stops it
(`OLO_SKIP_UI_LINT=1` to push regardless, saying why). A layout problem
found by looking that the lint missed is a missing check: add it, and
show it fires on the old layout.

`cargo ui-lint` runs every `ui_lint` test: the kit's own (a form of its
widgets is clean; controls put over each other are found) and every
Preferences panel at its size with made-up long display names and the
panels' test data (no app_server, no devices: their links answer with
it under `cfg(test)`), and Train for Your Palm full screen at 1280 × 800
and a MacBook's 1512 × 982, every step in every state
(`palm_trainer::lint_states`: fresh, a touch judged, the longest miss,
done, Esc pressed once; training; each of the summary's outcomes; the
try). `tools/install-session.sh` runs it and stops if it fails. A
window of a fixed size or a dialog gets a `ui_lint` test beside it:
build its view with the longest things it can show. A full-screen one
(`WindowOptions::fullscreen`) is centred in what the screen gives it,
so the fit check doesn't ask it to be shorter.

Go through each finding: fix it in the kit or the view, or, when it is
meant, tell the review why (`own_grid`, `band`) so the next run is
quiet.

Memory: measure release builds (`cargo build --release -p APP`, then
`grep -E 'VmRSS|RssAnon|RssShmem' /proc/PID/status`; RssShmem is GPU
buffers on Asahi). Baselines (October 2026, 2× headless): DeskCalc 54 MB
RSS (15 MB heap, 13 MB GPU); Terminal 79 MB idle, +29 MB for a full
10,000-line scrollback at 120 columns (alacritty's cells), +28 MB per
extra window (swapchain). GPUI's rules keep it there: fonts are mapped,
not copied (the CJK face is 31 MB), the GPU's allocator works in small
chunks, and windows share one device, its pipelines and its atlas
(gpui/README.md).

## Screenshot journal

`docs/journal/` is Olo's visual history (images in Git LFS). Whenever
something visible changes — a new app or window, a look-and-feel fix, a
theme — add an entry and commit it with the change:

```sh
tools/journal.py add mine "Deskbar: team menu" "What changed and why"   # from a tsession
tools/journal.py add /tmp/before.png "Menu padding before the fix"        # an existing image
tools/journal.py add mine "Tracker toolbar" --crop 40 30 600 440         # logical-pixel crop
```

For fixes, journal a before and an after.

## Commits

Small, focused commits with a descriptive message ending in the
`Co-Authored-By` trailer the session uses.

## Preferences (apps/preferences)

One process, one small fixed-size window per panel, as BeOS had a
preference app per area. `olo-preferences <panel>` opens or brings
forward a panel (Deskbar's Preferences menu does this); with no argument a
launcher lists them. The panel list Deskbar shows is
`olo_xdg::apps::PREFERENCE_PANELS`; a unit test keeps it in step with
`panel::Panel`.

On Olo's GPUI. The `Prefs` entity (main.rs) keeps the settings,
app_server's displays and power state, and the windows; each panel's
window is a `PanelView` (its panel, the settings as they were when it
opened, its own `panel::State` and text fields).

* Settings that live in app_server come over IPC (`server.rs`): a watcher
  thread follows `SettingsChanged` / `OutputsChanged`, a writer thread sends
  `SetSettings` (collapsed to the newest) and `ConfigureOutput`. A control's
  change goes to `Prefs::edit`, which sends the settings and draws every
  panel again; nothing blocks.
* Each panel module has `ui(ui: &mut form::Ui, ..)`: `form.rs` collects the
  panel's controls top to bottom, each bound to its part of the settings
  (`ui.check(label, value, |s, v| s.x = v)`, `slider`, `menu`, `choose`,
  `radio`, `text` with a text field kept by id, `bbox`, `row`,
  `enabled_if`, `note`); what a panel keeps of
  its own goes in `panel::State` and is changed through `ui.on_view`.
  A tall panel is two columns, side by side and each as wide as the
  other (`ui.columns(left, right)`, or `ui.collect` each side and
  `ui.add_columns` when both sides need the panel's state, as Sound's
  and AI's): boxes that belong together go in one column, and the
  window is widened (Sound 800, AI 760, AirPods 960) rather than made a
  scroll. Rows of choices in a column wrap (`flex_wrap`, `STACK` apart).
  Workers (Bluetooth, the clock, sound, the battery) keep what they read
  behind a lock and send `Msg::Redraw`. Windows open after the update
  that asks for them (`cx.defer`), as their panels read the settings
  while they're built. Panels that change settings get Defaults and
  Revert (to the values when the window opened) along the bottom, each
  enabled only when it would change something: implement `Panel::reset`.
  In those panels a box whose settings aren't their defaults has Reset
  at the end of its top line (`BBox::action`, a `LinkButton`), putting
  back just that box's: each control bound through `Ui` (`check`,
  `radio`, `slider`, `menu`, `choose`, `field`/`text`, and `bind` /
  `bind_click` for one built by hand, given the value it shows) leaves
  a probe in the boxes it's in, its setter with that value. Set on
  `Settings::default()`, a box's probes change the defaults only if the
  box was changed (`Settings`' `==`, then its JSON with floats to five
  digits, so a percent through f32 is no change: 6 µs); Reset copies
  where they change them back from the defaults, field by field and an
  item of a list at a time. A control bound with plain `ui.edit` isn't
  probed (an action, or a choice the defaults can't name: keyboard
  profiles). `cargo ui-lint` draws every resettable panel again with
  every box's Reset showing.
* Every panel's window follows what it shows (`fits_content`, "Windows
  fit what they show"): as tall as its rows, its `size` at least, and
  every panel with tabs has `TabView::grows()`. The lint draws them on
  a 1280 × 800-point screen beside the fixture's two (`lint::outputs`);
  every panel fits it unscrolled. Display has its settings and its
  panel's facts side by side (940 wide); Performance has two tabs
  (Supervisor, Watchdog: the thresholds two by two), and a button to
  Power, where the power mode is.
* New panel: add it to `Panel` (arg, title, icon, size, reset), write the
  module, add it to `PREFERENCE_PANELS`, journal it light and dark.
* **Default Apps** (`default_apps.rs`, BeOS's FileTypes), three tabs.
  *Default Apps*: each kind of file and link (`KINDS`: Text, PDF, Word
  documents, … Pictures, Movies, Music, … Folders, Web pages and links,
  Email links; each a few MIME types) with the app it opens in, a
  pop-up of the apps Open With offers for it
  (`olo_xdg::open::handlers_for`, of its first type), all one width; a
  choice is made for every type of the kind (`open::set_default`: for
  Olo and, in `olo-mimeapps.list`, for other programs). *Your Own
  Kinds*: the person's kinds (`olo_xdg::mime::own_kinds`: a name and
  its extensions), each with its app (any of `open::every_app`) and
  Remove; Add a Kind takes a name, extensions ("sav, sv2", dots and
  case don't matter) and an app; an extension another kind had is the
  new one's. *Open With*: for a kind (the built-in ones and the
  person's), the apps Open With ▸ offers, in order, a list with Move Up,
  Move Down and Remove (never the last), Add an App…, and Olo's Choice
  to let Olo choose again (`open::set_offered`, empty). Everything is
  worked out and written on a worker, which works it all out again
  after each change; "Looking for apps…" until then. Under `cfg(test)`
  made-up apps and one made-up kind, so the lint and the settings index
  don't depend on the computer (the person's kinds' rows aren't
  settings: the index leaves out what's in "Your Kinds").
* **Every setting, findable** (`olo_xdg::settings::PREFERENCE_SETTINGS`):
  each control in every panel (check box, radio button, slider, pop-up
  menu, text field, colour well, button) with its panel, tab and box,
  made from the panels as they're drawn (`Lint::trees`) by the
  `ui_lint_settings_index` test, which fails when a panel's controls
  change and the index wasn't remade: `OLO_WRITE_SETTINGS=1 cargo
  ui-lint` writes `crates/xdg/src/settings.rs`. Left out: a list's rows,
  scroll arrows, Defaults and Revert, a state shown as a button, a
  problem's fix, the made-up displays and devices (by their names;
  `generic_group` names their boxes as on any computer). The command
  palette lists them; `olo-preferences PANEL --tab TAB --group BOX
  --show NAME` opens the panel (or brings it forward) with the control
  lit (`olo_ui::spotlight`), once the window has drawn.
* **Integrations** (`integrations.rs`) is where every service Olo works
  with is connected, and only there: one box per service with its own
  icon (the service's mark made an Olo object, drawn for every theme as
  any icon), what Olo does with it, whom it is connected as and where
  the sign-in came from, and connect / disconnect; everything that
  touches the network or the keyring runs on a worker. An integration
  that can't work without a sign-in says so where it would have shown
  something and leads here. GitHub: `olo_system::github::token()` is
  the token kept by Olo (Connect with a Token… asks GitHub who it is,
  `GET /user`, and only then keeps it in the login keyring,
  `application=olo service=github.com`), else `GH_TOKEN` /
  `GITHUB_TOKEN`, else the GitHub CLI's, read and never run: in its
  `hosts.yml`, else in the login keyring (`service=gh:github.com`,
  username empty for the active account or the `user:` of hosts.yml;
  `go-keyring-base64:` values decoded). Test the keyring parts with
  `tools/dev/private-keyring` (`github::tests::tokens_from_a_keyring`).
* Tracker (`tracker.rs`) sets `Settings::tracker` (the view new folder
  windows open in); its icon is Tracker's own, as in Haiku.
* Terminal (`terminal.rs`) sets `Settings::terminal`: where New
  Worktree… makes a repository's worktrees (`worktrees`, `~/worktrees` to
  start, each repository a folder of its name in there), and Coding
  Agents' "Tell coding agents about the repositories in their tab group"
  (`agent_context`, off; see Terminal's **Coding agents' context**) and
  "Let coding agents keep their tab's title current" (`agent_titles`,
  off; Terminal's **Coding agents' tab titles**) and "Tell coding agents
  to use Olo's Agent Screens" (`agent_screens`, off; Terminal's **Coding
  agents and Agent Screens**).
  Terminal follows them live (`settings::watch`).
* Windows (`windows.rs`) sets the space Olo leaves around the windows it
  arranges (`Settings::tiling`: margin at the display's edges, gap
  between windows), with a miniature of the main display tiled three
  ways that follows the sliders. app_server refits arranged windows as
  they change, so the sliders move them live. Deskbar's Window
  Arrangement menu ends with Windows Preferences….
* Network (`network/`) reads NetworkManager through
  `olo_system::network` on a worker thread (devices, addresses,
  networks in range merged by SSID, saved connections; a scan every 30 s)
  and joins (saved connection, or a new WPA-PSK one), disconnects and
  forgets. Under the networks, **Hotspot**: a name (the computer's) and
  a password, Start (`nm::start_hotspot`: WPA2, `ipv4.method shared`,
  kept as the "Olo Hotspot" connection), and while it's on, what it's
  called and Stop. Its tabs (`TABS`; `olo-preferences network --tab
  Ethernet` opens at one, as Deskbar's link icons do):
  - **Ethernet** (`ethernet.rs`): each wired link (an Adapter menu with
    several), No cable / Connected with “…”, 1 Gb/s, Connect or
    Disconnect (`nm::connect`: its saved connection, else a new one), and
    its connection's IPv4: Using DHCP (the address and router DHCP gave
    shown, dim) or Manually (address/prefix, router), and DNS servers of
    its own, Configure IPv6 (Automatically, Link-Local Only, Off; one
    set Manually elsewhere is left as it is) and the MTU; Apply writes the
    connection (`nm::update_profile`, `set_addressing_in`: `Update2`,
    secrets kept) and `Reapply`s the link, so it changes without
    disconnecting. NetworkManager lets wheel users on
    the console change system connections themselves; for anyone else it
    goes through the root helper.
  - **VPN** (`vpn.rs`): the VPNs saved (WireGuard, and NetworkManager's
    plugins': OpenVPN, IPsec…; `nm::vpns`), each Connected or not,
    Connect / Disconnect, Remove (asked first: its keys go with it), and
    Import WireGuard… (a file panel): `wg-quick`'s `.conf` read natively
    (`nm::wireguard_settings`: keys checked as 32 bytes of base64,
    addresses by family, DNS servers and search domains, the DNS asked
    first and alone while it's up, `dns-priority` -50, so a full tunnel's
    names don't leak; unit-tested) and saved (`AddConnection`).
  - **Service Order** (`order.rs`), as a Mac's Set Service Order: Ethernet
    and Wi-Fi in the order traffic prefers them, the one in use checked,
    Move Up / Move Down, Use the Usual Order, Apply.
  - **DNS** (`dns.rs`): servers and search domains for every link, a menu
    of public resolvers (Cloudflare, Google, Quad9, with their IPv6
    addresses) to start from, and what each link asks now.
  - **Hosts** (`hosts_tab.rs`): /etc/hosts's entries (address, names,
    comment, on or off), the machine's own (localhost) dim and not
    editable; Add, Remove, the fields edit the entry chosen, Save writes
    them all.
  - **Shaping** (`shaping_tab.rs`), as a Mac's Network Link Conditioner:
    a link, what it receives and what it sends each with bandwidth,
    delay, jitter and loss, a menu of networks to start from (Edge, 3G,
    LTE, DSL, Busy Wi-Fi, Very Bad Network, Offline), Turn On / Apply and
    Turn Off, and how it's shaped now (read back from the kernel).
  - **Details**: each interface's addresses, router, DNS, hardware
    address and speed.

  What needs root goes through **`olo-net-admin`** (crates/system,
  `admin.rs` in the panel): started through pkexec (Olo's polkit agent
  asks for the password) the first time a change needs it, then kept
  and fed requests as JSON lines on stdin while the panel is open (the
  password asked once), let go as the panel closes. It writes the
  services' order and DNS of one's own into NetworkManager's own
  configuration, `/etc/NetworkManager/conf.d/90-olo.conf`
  (`olo_system::netconf`: each kind of link's route metric by default,
  `[connection-olo-wifi]` `match-device=type:wifi`, so a network joined
  later by anything falls in line; NetworkManager's global DNS,
  `[global-dns]` and `[global-dns-domain-*]`, which takes the place of
  each network's; search domains given without servers go onto each
  connection instead, since global DNS without servers would leave every
  link with none, and the file remembers them to take them off again),
  then `Reload`s NetworkManager and `Reapply`s every link connected; it
  writes /etc/hosts in place (`olo_system::hosts`: the file keeps its
  inode, owner, mode and SELinux label; comments and untouched entries
  stay as written; an entry turned off is `#off …`; refused if the file
  changed since it was read, and every line checked); and it shapes a
  link (`olo_system::shaping`, rtnetlink spoken directly: a `netem`
  queue at the link's root for what it sends, and for what it receives
  an `ifb` device of Olo's, `olo-ifbN`, fed by a `matchall` filter with a
  `mirred` redirect on the link's ingress, with its own `netem`;
  clearing takes away only Olo's). Everything it's sent is checked
  before it acts (`NetConf::check`, `HostsFile::check`, `Shape::check`).
  Tests: `OLO_NET_ADMIN=direct` runs it as this user, `OLO_NET_ROOT=DIR`
  puts /etc under DIR, and `OLO_NM_BUS` points at `nm-fake` (which reads
  the conf file at `Reload` and applies its metrics at `Reapply`, as
  NetworkManager does): `tsession run NAME OLO_NM_BUS=… OLO_NET_ROOT=…
  OLO_NET_ADMIN=direct olo-preferences network --tab Ethernet`. Shaping
  was tried as root in a network namespace of its own (`sudo unshare -n`,
  `cargo run --example shape`: `lo` sent 100 ms later pinged 200 ms,
  received 50 ms later 100 ms, both 300 ms, 50% loss received lost 72%
  of pings, both ways crossing it; cleared, as before). A new enterprise (802.1X) network's Join… opens a sheet
  (`EnterpriseForm`, main's `Win::Enterprise`): PEAP / TTLS with an inner
  method, username and password, or TLS with a certificate and key
  (`Win::Cert` file panels, `CERT_TYPES`); anonymous identity; the CA
  (system certificates, a file, or none) and a domain suffix. It resizes
  to its rows. `nm::enterprise_settings` builds the `802-1x` settings
  (unit-tested); secrets are zeroed after use. **Tests only look: never
  toggle Wi-Fi, Join, Disconnect or Forget on the real machine.**
  `OLO_FAKE_NETWORKS=1` (debug builds; pass it with `tsession run NAME
  env OLO_FAKE_NETWORKS=1 …`) shows made-up networks instead and
  pretends to join, so the panel and the sheet can be driven headless.
* Bluetooth (`bluetooth.rs`) reads BlueZ (`olo_system::bluetooth`,
  ObjectManager) every second, searches for 30 s when it opens or on
  Search, lists named or paired devices, and pairs, connects, disconnects
  and removes on a thread per action. Its pairing agent is registered for
  this process only (never `RequestDefaultAgent`) and shows passkeys and
  PIN requests in the panel. **Tests only look and search: never Pair,
  Connect, Remove or toggle Bluetooth on the real machine.**
* Touch ID (`touchid.rs`) lists, enrols (`fingerprint::enroll`, with a
  StatusBar per touch, cancellable) and removes fingers through fprintd.
  fprintd only answers a process in an active local session; headless, the
  panel says so. `OLO_FAKE_FINGERS=right-index-finger,…` (debug builds)
  fakes a sensor so the whole panel can be exercised headless. **Never
  enrol or remove on the real sensor in tests.**
* Notifications (`notifications.rs`) edits `NotificationSettings`, its
  own file rather than app_server's `Settings` (so no Defaults/Revert): a
  watcher thread reloads it, and the apps from the history, when either
  changes; edits save on a thread (echoes of older files are ignored for
  a moment after an edit). Times follow the clock's 12/24-hour choice.
  Its History box: "Fade notifications away over" a `MenuField` of
  durations (15 minutes to a week, and whatever the file says if it's
  none of them), a strip painting the fade from Now to gone (dimmed,
  not hidden, with fading off, so nothing moves) and a line saying what
  happens to them; and "Only the latest of the same notification"
  (`one_of_each`, on by default): olo-notifications, as one arrives (not
  an app's own replacement), takes away any still active (a banner up,
  or unread and not pinned in the history) from the same app with the
  same title and text (`store::same_as`), telling the app it expired.
* Backup (`backup.rs`) exports every Olo setting and piece of saved
  state (`config_dir()` and `state_dir()`, as `config/…` and `state/…`;
  not the automatic backups nor Agent Screens' own homes, `LEFT_OUT`)
  into one `.olo-backup` file: a zstd tar with `manifest.json` first
  (format 2: version, when, each file and its size, where it was made,
  `Origin`: the home folder, a digest of the machine's id and the
  user's, the processor). Sealed with a password (the default, "Include
  passwords and sign-ins"), it also holds the secrets Olo keeps in the
  login keyring, every one of `olo_system::secrets::KEPT` (the keys to
  Passwords' and the Token Pool's vaults, the GitHub token, Telegram's
  API ID and sign-in: each owner finds its secret by its entry there,
  so one added goes there or a backup leaves it behind), and its files
  and those go in a tar of their own, sealed (`Seal::Passphrase`:
  Argon2id, 64 MiB and three passes, then XChaCha20-Poly1305 a
  megabyte at a time, each piece numbered and the last marked, so none
  can be dropped, moved or cut off unnoticed). Open, it holds no secret.
  What goes in, and what comes back, is chosen a part at a time
  (`Choice`, the parts left out; Ben: "maybe i have an export with
  everything but i just want to include token pooler settings/accounts
  for my import"): each app's files (`app_of`) with the keyring's
  secrets that go with them (`part_of_secret`: the Token Pool's key
  with its accounts, Telegram's sign-in with Telegram, GitHub's token a
  part of its own) and the Wi-Fi networks (`WIFI_PART`). The Export box
  lists this computer's (`parts_here`, read on a thread as the panel
  opens and after each export and import), each a check box with what
  it holds, those only a sealed backup holds disabled while it isn't
  sealed; the Import box lists the chosen backup's (`Manifest::parts`).
  Each list has Select All and Select None over it (`all_or_none`), and
  the two boxes stand side by side (one over the other, the panel was
  taller than a 1280×800 screen).
  Both lists build only the rows they show (`shown_rows`: a control
  scrolled out of sight would still say it's there to agents, and the
  palette's settings index would take it for one of the panel's), each
  row a `ListItem` to agents. `export_only` and `import_only` take the
  choice; what's left out of an import stays as it is here, and what's
  here is still all saved first.
  Import reads the manifest and shows what it holds by part in the
  Import box's list (`Manifest::parts`), asks for the password if it's sealed, and opens it before
  replacing anything (a wrong one changes nothing); it then saves what
  is there to `state_dir()/backups` (the last five; sealed with a key
  kept in the login keyring, `Seal::Keyring`, secrets and all, so an
  import can be undone by importing that, here only), replaces each file
  atomically, keeping its mode (names outside the two roots are
  refused; files the backup lacks are left alone), and puts its secrets
  in the keyring. Made on another computer (`Manifest::from_elsewhere`),
  a backup is moved in: paths under the old home folder are moved to
  this one's (`rehome`: where it's a path's start and a whole name),
  displays known by their port rather than their monitor are left out
  of app_server's settings (`without_connectors`,
  `olo_protocol::is_connector_key`: on another computer "eDP-1" is
  another panel), Telegram's sign-in stays behind (each computer signs
  in as itself: `Carry::SameComputer`), the Token Pool's accounts come
  in paused (`pool-arrived.json`, which the pool reads as it opens its
  store: each sign-in renews itself, and two computers renewing one sign
  each other out), and a vault whose key the backup lacks stays as it is
  here. What didn't come along, or came differently, is said
  (`Imported::notes`: also Bluetooth pairings and Touch ID
  fingerprints, the computer's own, and a Mac keyboard profile).
  A sealed backup also takes the saved Wi-Fi networks along, with their
  passwords (`olo_system::network::saved_wifi`: NetworkManager's
  `GetSettings` and `GetSecrets`, which ask polkit, so Olo's agent may
  ask once; enterprise ones stay behind, set up with the computer's own
  certificates, and are named), in `wifi.json` beside the secrets;
  import saves those this computer hasn't (`add_wifi`, a network to
  join, not joined) and keeps its own. The backup made before an import
  reads none (an import only adds networks). Backup reaches the keyring
  and NetworkManager through `Here` (`Keyring`, `Networks`), which its
  tests make of their own; `nm-fake` (in olo-system: a stand-in
  NetworkManager on any bus, saved networks from `--network
  SSID:PASSWORD` and `--enterprise SSID`, what it holds written to
  `--dump FILE`) and `OLO_NM_BUS=ADDRESS` test the D-Bus side without
  touching this computer's networks. It has links too: `--wired` (an
  Ethernet adapter connected), `--wifi SSID` (an adapter joined to it),
  `--ap SSID:STRENGTH` (more in range), with addresses, routers and DNS,
  the default route where the lowest route metric is, profiles' `Update2`
  and devices' `Reapply`, signalled as NetworkManager does; `nm-fake
  --address A --tell "wired down"` (`wired up`, `wifi down`, `wifi up
  SSID`, `strength N`, `state`) changes the world under a test. Run
  Deskbar or Preferences against one with `tsession run NAME
  OLO_NM_BUS=ADDRESS olo-deskbar`, and `cargo run -p olo-system
  --example links` prints the links as the watch sees them (`--time`:
  how long a read takes). No
  login keyring: no secret is read or put back, and the vaults stay.
  app_server keeps its settings in memory, so the backup's
  `app_server.json` is sent to it as `SetSettings`. Both run on a
  worker thread with progress and Cancel (import can only be cancelled
  while it saves what is there). `olo-preferences --export FILE
  [--sealed]`, `--inspect FILE` and `--import FILE` do the same without
  a window (a password read from stdin; `--only "Token Pool,Passwords"`
  for some of the parts `--inspect` lists, in any case); opening a `.olo-backup`
  (`mime::SETTINGS_BACKUP`, kit `Kind::SettingsBackup`) shows it in the
  panel, ready to import (`backup::open_later`: the panel's window may
  be made after). Tested with a real keyring on a private bus
  (`tools/dev/private-keyring`, and Passwords' ignored
  `through_a_backup` test making and opening a vault on each side).
* Date & Time (`datetime.rs`) talks to systemd-timedated through
  `olo_system::time` on its own worker thread (status every 2 s; set
  time, zone and network time with polkit's interactive auth, so a
  password prompt never holds the window). Zone offsets and times come
  from jiff and the system tz database. Deskbar's clock format is
  `Settings::clock` (`ClockSettings::format` is shared by Deskbar and the
  panel's preview). **Tests must never press Set, Set Time Zone or the
  network-time box: they change the real machine.**
* Backgrounds (`workspaces.rs`): a colour per workspace and one wallpaper,
  picked in a `ThumbnailPicker`: None (the workspace colour), the
  wallpaper pack, or Your Own. The pack (`data/wallpapers`, Git LFS;
  `olo_xdg::wallpapers`) lists its pictures in `pack.txt` (the first
  is a new install's default), each in `4k/` (16:9) and `ultrawide/`
  (21:9), some with a `-dark` version, and previews in `thumbs/`. Each
  theme's own wallpaper leads the list, named after it (see "Theme
  wallpapers" in [ICONS.md](ICONS.md)); choosing a theme in Themes sets
  `wallpaper` to it too, so an agent switching looks over IPC sets both
  `appearance.icon_theme` and `wallpaper` (`themes::wallpaper(id)`). The
  setting holds `pack:NAME`, and `wallpapers::resolve` gives the file
  for a screen's shape (2:1 or wider takes the ultrawide one) and the
  effective theme, so it follows dark mode (Auto's schedule too) live.
  It is installed into `~/.local/share/olo/wallpapers` by
  install-session.sh and by upgrades (cloned on btrfs), and found in
  the recorded build tree's `data/` too (`data_dirs`). Choose… opens a
  FilePanel (picture types only) as a child window for your own file;
  its text field follows the setting when it changes elsewhere. Each
  pack picture has an HDR version: a gain map beside it
  (`NAME.gain.png`, `olo_images::gain`), greyscale at half size, how
  many stops brighter each point is (0 to `MAX_STOPS`, 2: 4× SDR's
  white), made from the picture by `wallpaper-hdr` (points that stand
  out from what's around them, light sources, LEDs, the moon, glints,
  shine most; near-white glows some; large bright surfaces stay at SDR's
  white or within a quarter stop, so nothing glares). 7.5 MB for all 56,
  against gigabytes for 16-bit HDR copies. Backgrounds says when the
  chosen picture has one (`wallpapers::has_hdr`). app_server draws the wallpaper (`wallpaper.rs`):
  each output's picture is decoded (JPEG XL too, via jxl-oxide), scaled
  to cover and cropped on a worker thread (on a display in HDR, its gain
  map scaled and cropped alike and multiplied in, `gain::apply` through
  a 64K-entry table, into half floats of linear light, 1.0 SDR's white,
  drawn with that colour source, `Exact::with_color`: its lights shine
  up to the display's peak and "Everything else's brightness" moves the
  rest with no new texture; at 3200 × 2000, 94 ms and 48 MB against SDR's
  73 ms and 24 MB, off the loop, `wallpaper_cost`; `hdr-check` reads its
  lights above SDR's white on the signal) (one picture at a time, only
  the latest asked for each output), kept in the output's user
  data and drawn under every surface; the workspace colour shows until
  it is ready or if the file can't be read. The worker cuts it into
  strips of about 4 MB, each a texture of its own, and a new picture is
  uploaded a strip a frame while the old one stays up, then swapped in
  whole: a screen's whole picture (30 MB at 3456×2234) uploaded in one
  frame held it up 20 ms. Window tabs likewise: a tab whose look changes
  is drawn on a worker (`deco.rs`, `tab_worker`: cosmic-text's shaping
  and glyphs took 4–13 ms at times) and the old one shown till it's
  done; a window's first tab is drawn at once. A frame over a 120 Hz
  frame's time logs where its time went (gathering or drawing, and the
  window, layer or wallpaper that took it; the elements new to it, the
  area drawn, and the thread's CPU time, waits, preemptions and page
  faults, `render::ThreadUse`, which tell work from waiting).
  New GPU memory is what held frames up most (Apple's GPU, Mesa's
  Asahi driver, sampled with `eu-stack` in the act): mapping it for the
  GPU (`agx_bo_bind`'s ioctl) took 10–33 ms of CPU at times while the
  GPU was busy. Mesa keeps what frames and uploads used for a second or
  two and hands it out again, so app_server keeps that warm and stays
  within it: a transparent pixel in each screen's corner shows for a
  frame every second while the desktop is in use (`render::keep_warm`,
  not while idle for two minutes, and only for a frame, so a
  full-screen window still goes straight to the display; smithay counts
  an element's place from the top of the stack, so its coming and going
  damages what's under it whole: a headless screen's watchers compare
  what a frame shows, `render::content_changed`), with an upload
  of a spare tab buffer of each size (`deco::warm_tab_upload`); tab looks
  are drawn into buffers of two widths, written whole, taken from spares
  made for each tab height at its first window (`SPARE_TABS`), so a new
  look needs no new memory; and headless screens draw the next frame
  only once the GPU has finished the last (`headless::Screen::drawn`),
  as a display would take it, rather than piling frames up. Scaling is in 8 bits with
  the crop applied as it scales (`fast_image_resize`), one full-size copy
  at a time, and the memory it took goes back to the system afterwards
  (`malloc_trim`), so switching themes doesn't leave app_server hundreds
  of megabytes bigger.
* Display (`display.rs`): a map of the screens; drag one and `snap` puts
  it edge to edge with the nearest other (sharing at least 64 px, lined
  up with edges or centres when close), and every screen's position is
  sent. app_server's `shell::arrange` places outputs at their saved
  positions (overlaps fall back to a row) and normalises the layout to
  start at (0, 0); a screen that changes size takes the screens on its
  right and bottom edges with it. Resolution and refresh rate go through
  `Backend::set_mode` (DRM `use_mode`; headless recreates its target),
  are saved per output and restored at start-up; a new one reverts after
  `KEEP_SECS` unless Keep is clicked. Rotation is the picture turned
  clockwise (`desktop::rotation_transform`). The main display
  (`DisplaySettings::main`, else the first) gets layer surfaces that name
  no output (the desktop): they carry `shell::AutoOutput` and move when
  the main display changes or their output is unplugged. One that comes
  before any display (DRM finds them after the clients are back from a
  restart) waits in `homeless_layers` for the first, not closed (Deskbar
  and the desktop, with no window left, quit, and olo-session gave up
  restarting them). A display coming or going is told at once
  (`OutputsChanged` from `displays_changed`; before, only the power
  tick's next look at the external displays told it, up to 5 s on, and
  Deskbar's bar on a display back after an upgrade came that late, with
  the tiles there laid out under where it would be). Those called up
  from the keyboard to work where the person is
  (`shell::OPEN_WHERE_IN_USE`: the palette) open on the display in use
  instead, `active_output()`: the focused window's, else the display a
  workspace was last switched on or the pointer last pressed on
  (`AnvilState::last_display`), else the pointer's. Every display has
  its own Deskbar (one layer surface per output, `Bar` in apps/deskbar,
  each naming its output: one that named none would follow the main
  display about; the displays asked for again once subscribed to
  events, so one that came in between isn't missed), each listing every display's workspaces and their
  applications, its own display's first, then the others left to right
  (`displays_from`; a window goes by `WindowInfo::output`, one whose
  display isn't known counting as the main one's), and the map draws a
  Deskbar strip on each. With several displays its tray shows its own
  display's number in a small monitor (`status::draw_display`, drawn as
  Agent Screens' monitor is); clicking it lists the displays, its own
  checked (`display_menu`), and choosing one goes there
  (`Request::FocusDisplay`: the pointer to the middle of where windows
  go on it, the keyboard to its front window, else its desktop, so it is
  the active display). With more than one, each display goes by a
  number from the main display (`olo_protocol::display_number`): the
  main one is D0, the displays left of it D−1, D−2, …, those right of
  it D1, D2, … (left to right, then top to bottom), so a number says
  where a display stands and changes only when the displays are moved
  about or another is made the main one (Ben, 2026-11-01; before, each
  monitor kept a number of its own for good, which came out stale when
  a display was set up by its connector before its EDID arrived). The
  map's and the settings box's names ("D1 Studio Display"), Deskbar's
  tray and menus and its workspaces' names (`workspace_name`: "D−1
  Workspace 2") all use it; `display_label` and `display_number_text`
  spell it, with a minus sign.
  render.rs draws the layers itself, front to back: Overlay, the menus
  of the Top, Bottom and Background layers (Deskbar's, the desktop's
  right-click menu: `shell::layer_menus`), the windows' menus (their
  popups, `WindowElement::menu_elements`, gathered with the windows and
  drawn apart from them: `render_parts` without), the Top layer's
  surfaces (Deskbar, notification banners), the windows, then the
  Background and Bottom surfaces; `surface_under` and keyboard focus
  find them in the same order (`layer_menu_under`, `window_menu_under`),
  so a desktop menu opened over a window is in front of it and takes
  its clicks, a window's menu opened by Deskbar is over Deskbar (it went
  under), and Deskbar's menus are over a banner that came after it (a
  layer surface's menus went with it, under any later one).
  Identify (`IdentifyOutputs`) shows each display's name for 4 s
  (app_server `identify.rs`). Settings are kept per monitor under
  `olo_protocol::display_key` ("make model serial" from the EDID,
  else the connector; `OutputInfo::key`), so they follow a monitor to
  another port; always go through `DisplaySettings::config` /
  `config_mut`, which also read and migrate older connector entries.
  Mirroring: `OutputConfig::mirror` names the source's key (`Some("")`
  means extend). `desktop::apply_mirroring` takes a mirror out of the
  Space into `State::mirrors` (so windows, the pointer and layers never
  land on it; `OutputInfo::mirror_of` reports it) and the backends draw
  the source's elements on it through `render::FrameElements::Mirror`,
  scaled to fit and centred (`render::fit`). Preferences hides a mirror
  from the arrangement and offers only mode, refresh and "Use as". The
  layer surfaces that named the mirror are closed as it goes
  (`move_auto_layers`); one going away is looked for in every display's
  layer map, the mirrors' too (`layer_destroyed`), or the dead one stays
  in the mirror's map and is still there when it extends again (a
  Deskbar bar left there, empty, moved the tiling area over: so the
  usable area goes by a Deskbar layer that is drawn, `usable_area`).
* Memory buffers drawn by app_server (twitcher, wallpaper, identify) must
  pass the full buffer as `src` to `MemoryRenderBufferRenderElement::
  from_buffer`; without it the buffer counts as logical pixels and is
  drawn at twice its size on a 2× screen.
* Themes (`themes.rs`): a row per theme in `olo_xdg::themes::THEMES`,
  each drawn in that theme (`icons::paint_in`, `brand::wordmark_in`):
  a radio button with its name, a line about it, its wordmark and six of
  its icons; clicking anywhere on a row chooses it, and its wallpaper
  (`choose`). Beside the rows, the chosen theme's colour sets one above
  the other (its own, BeOS's, Catppuccin's: a preview of each in light
  and dark), on a grid of their own (`review::own_grid`): side by side,
  the panel fits 1280 × 800 with up to a dozen themes. The choice is
  `Appearance::icon_theme` (an id; unknown ones draw the default,
  `bebox`), so agents set it with `SetSettings` like any other.
  Every process draws in one theme, `olo_xdg::themes::current()`: the
  kit sets it from the settings before the first frame
  (`theme_watch::initial_palette`) and on every `SettingsChanged`
  (`Kit::set_icon_theme`, which redraws every window; the icon and
  wordmark caches are keyed by theme and drop the old one's), and
  app_server sets it on load and in `set_settings`, where Twitcher looks
  its apps' icons up again (`twitcher::theme_changed`). Outside Olo,
  `OLO_ICON_THEME=id` picks one. `find_icon` looks in the theme's
  `themes/<id>/icons` in every data directory, then in `icons`.
* About (`about.rs`) has Restart Olo: SIGHUP to `$OLO_SESSION_PID`,
  the same in-place reload as `olo-session reload` (disabled outside
  a session).

Appearance ▸ Longest window title is `Settings::tab_title_chars` (100):
app_server's decorations cut a title to it with an ellipsis
(`deco::cut`, set by `deco::set_title_chars` on load and on every
settings change) before measuring and drawing the tab. A title a tab
cuts short (by that, or by its room) shows whole on a tooltip's plate
under the tab once the pointer rests on it (500 ms, as the kit's
tips), at once while ⌘ is held, as the kit shows the text it cuts
(`tabtip`: `Decoration::cut_title` says where a cut title is, the
pointer's motion over the tab arms one timer, the plate is drawn with
the display's frame over all but the pointer, slid to stay on the
display; a press puts it away until the pointer leaves). The motion
comes inside smithay's pointer handle, which holds its lock: nothing
there asks the pointer (where the tip goes is worked out as it shows).
app_server's other cut text uses the same plate (`tabtip::plate`):
Twitcher's chosen label, cut to the panel, shows whole on it under the
panel at once (⌘ is held), drawn with each look and kept with it
(`Overlay::elements`, `cut_label`); the overview's chosen window has
its title whole on it at its foot, as wide as the title needs, past the
window's sides and slid to stay on the display, cut only past the
display's width.

Appearance ▸ Show icons in the Deskbar menu is
`Appearance::menu_icons` (on): Deskbar builds its Olo menu with an
icon on every item (`OUR_APPS` and `PREFERENCE_PANELS` icons, each
.desktop entry's, a sunburst icon per command and category) and strips
them all when it's off.

The Olo menu also has the desktop-wide commands app_server's shortcuts
do: Window Arrangement (the windowing modes, `mode:` and its id; Tile
All, Smart Tile, Zoom All, Cascade All, Fan Out, Cascade / Fan Out
Others, Undo), Workspaces (go to one, Move Window
To, Workspaces preferences) and Show or Hide All Windows. Each is
`arrange:` and a shortcut id, sent as `Request::RunShortcut` naming the
window that last had the focus (`last_focused`: opening the menu takes
the keyboard, so app_server sees no focused window by then).
Each item shows the keys the person has for it (`add_keys`:
`shortcuts::keys_for` with the keyboard profile, as its platform writes
them, sent on every settings change).

Every display has workspaces of its own (see "Workspaces, one set per
display" below), and each Deskbar groups the applications as a
BOutlineListView would group them, display by display (its own first),
every workspace of each display with windows and the one it shows (even
if it has none), always in order:
each under a heading naming it (`workspace_heading`, `HEADING_H`; "D1
Workspace 2" with more than one display, `Deskbar::workspace_name`),
with only the applications that have windows there, each row's menu
only those windows (`teams(display, Some(ws))`). A heading is a quiet caption, bold
in the small size and dimmed toward the panel, in line with its
applications' icons and the tray's date (`ICON_INSET` = `TRAY_TEXT`).
The workspace showing is lit as a whole, heading and applications, as a
BListView selection as big as the group: its ground is
`style::lit_ground(palette, panel_background)` across the bar, a
hairline of the selection's edge above and below (`style::lit_edges`,
painted after the rows), its heading in the selection's `text`. The
rows in it shade that ground for their focus, hover and separators
(`Deskbar::ground`) as the others shade the panel's, so it follows every
theme and colour set, light, dark and night; the other workspaces stay
on the plain panel.
Clicking a heading goes to that workspace on its display
(`SwitchWorkspace` with `output`); on the one showing it opens the
Workspaces menu (that display's workspaces, and Move Window To). The
right button opens the heading's menu (`heading_menu`,
`heading_command`): Show All, Hide All and Close All for its windows
(`ActivateAll`, `Minimize`, `Close`), Move to the same workspace of each
other display, and Move All Windows To any workspace of any display
(`Request::MoveWorkspace`). At its right end is that workspace's
windowing mode (`status::draw_mode`: two windows one over the other for
Standard, tiles for Auto-Tiling, a viewfinder for Focus, lit while it
isn't Standard), there rather than in the tray so it reads as the
workspace's: a click opens the modes for that display's workspace
(`modes_menu`; for the one showing, all of Window Arrangement,
`arrange_menu`), `menu_output` and `menu_workspace` saying which. Agents
find the headings as `Button`s ("D1 Workspace 2", `selected` for the
one showing).
The tray's status icons (upgrade, agent screens, battery, Ethernet,
Wi-Fi, volume, notifications, the Caffeinated cup and the approval key,
`status::draw_key`, `approve.rs`: see "Approving without asking") sit on
a 22 px grid in a recessed well, in as many rows as they need, shared
out evenly among them and each row centred (eight are two rows of
four).

The links: Ethernet (a port, `status::draw_ethernet`, its notch and
contacts centred in device pixels at every scale) while a wired link is
connected, Wi-Fi while there's an adapter. With both connected, the one
not carrying the traffic (NetworkManager's primary connection, where
the default route goes) is drawn faint (`with_element_opacity`), its
tooltip saying it stands by. A click opens Preferences ▸ Network; a
right-click the link's menu (`link_menu`), as a Mac's Option-click on
Wi-Fi: what it's joined to, In Use for Internet Traffic or Standing By,
the signal and rate (Wi-Fi) or speed (Ethernet), IP address, router,
DNS servers, interface, hardware address, bytes received and sent
(`network::traffic`, the interface's counters, read as it opens), and
Network Preferences…. The addresses and rates aren't in `Status` (the
tray doesn't draw them, and a rate changes every few seconds): the
watch keeps them aside (`status::links`) for the menu to read.

Other programs' tray icons come first in the well (`sni.rs`): Deskbar is
the session's StatusNotifierWatcher (`org.kde.StatusNotifierWatcher`,
where Slack's, Discord's, Telegram's and Ayatana items register: by bus
name, or by object path on their connection) and its host (if another
program has the watcher's name, Deskbar hosts that one's items instead).
A worker reaches the bus, holds the items, and reads each one's
properties (picture, title, tooltip, status, `ItemIsMenu`) and its menu
(`com.canonical.dbusmenu` `GetLayout`, every level) ahead of time, each
read on a thread of its own so an app that doesn't answer holds up
nothing; it reads them again on their change signals (`NewIcon`,
`NewToolTip`, `NewStatus`…, `LayoutUpdated`), and drops an item when its
bus name goes. Passive items aren't shown, and one needing attention
shows its attention picture. A picture is the theme icon it names (or
the file in its `IconThemePath`), else the pixmap it sent: the smallest
at least 16 points of device pixels, else its biggest, scaled (alpha
premultiplied) to exactly that size and kept per item, picture and
size, so it is drawn a texel to a device pixel at any scale. Its tip is
its title and tooltip; Electron's have no title, so they take their
program's name (the owning process's executable, named as its row would
be: `app_name`). A click activates it (opens its menu, for one that is
only a menu), the right button opens its menu as an Olo menu (the
entries' mnemonics gone, checkmarks and radio items as checks, disabled
and hidden ones as they say, submenus as submenus) and tells the app
(`AboutToShow`), a choice goes back as its `Event … clicked`, the middle
button is its secondary action, and scrolling over it scrolls it. Items
re-register by themselves when Deskbar restarts (they watch the
watcher's name). The same worker hears launcher badges
(`com.canonical.Unity.LauncherEntry`'s Update signal, which Chromium,
Electron and Telegram send: `application://slack.desktop` with `count`,
`progress`, `urgent` and their `-visible`s, each update changing only
what it names), kept per desktop entry until the sender leaves the bus
and matched to a row as notifications are (`same_app`). An unread count
or a wish for attention lights the row's dot (no number, as for
notifications), a count going up or attention newly wanted shakes it,
and progress is a 2-point bar along the row's foot from the name to the
right inset, a device pixel above the row's line, in
`keyboard_navigation` on a faint track; agents read "3 unread, 40% done"
as the row's value. To try it, run a throwaway Slack in a test session
(sandbox HOME and `--user-data-dir`, `--proxy-server=127.0.0.1:9` so it
reaches nothing): its icon and whole menu show.

An app with notifications unread has a dot at its row's right end
(`NOTE_DOT`, 7 points, on device pixels: `style::snap_centred`), in
the colour the history marks unread ones with (`keyboard_navigation`,
not an alarm's red; a selected row's text colour on a selected row),
its name shortened to keep clear of it. Notes are matched to the row by
the desktop entry they name (an Olo app's `olo-terminal` is window
`olo.terminal`'s; another's, its entry's file or `StartupWMClass`), else
by name (`same_app`). One arriving (its `(time, id)` newer than any
Deskbar had seen; what is unread when Deskbar starts was there before
it) shakes the dot side to side for 0.6 s, 2 points, settling as it
goes (`SHAKE`), unless Do Not Disturb is on. As the app's notifications
fade (the history's fade setting) the dot drains from its right: it is
filled from its left to how fresh they are together
(`NoteApp::freshness`: each counts 1/n, its `fade_strength`; a pinned
one stays whole), never below `NOTE_DOT_FLOOR` (0.3) so it still reads
as a dot, the rest a faint well of its colour (`NOTE_DOT_WELL`), its
edge on a device pixel; repainted every 20 s while it isn't full.
olo-notifications puts in `NotificationState` each app's `count` and
when each of its fading ones came (`fading`), and the fade's length.
With Preferences ▸ Notifications ▸ Deskbar ▸ "Show how many beside each
app's dot" (`deskbar_count`, off by default) the count goes beside the
dot in the small type and the dot's colour ("99+" past 99; the name is
cut short first). Agents read the row's value, "has notifications (2,
the dot 60% full)".

Clicking an app's row in Deskbar brings it forward with every window it
has there, as a Mac's Dock does (BeOS's brought one forward):
`bring_forward` sends `Request::ActivateAll`, which brings them back
(minimized ones too, going to their workspace) in the order they were
used, so the one used last ends in front with the focus. The row's
menu's Show All does the same. The right button (or a long touch) lists
its windows (`team_menu`), as holding the button did in BeOS's Deskbar:
the focused one checked, one on another workspace with that workspace's
number, and a minimized one marked as a Mac's Window menu does, a hollow
diamond and a lighter label (`.minimized(true)`, `Action::minimized`;
agents read `minimized` on its `MenuItem`). Choosing one brings it back.
Below Hide All, Show All and Close All it moves the app's windows (those
the row stands for) as a window's tab menu moves one: Move to Workspace ▸
(Move All to Workspace with several), every display's workspaces left
to right, the one they're all on checked (`move:N:OUTPUT`, a
`MoveToWorkspace { output }` each, in turn on one connection,
`requests`), and with several displays (All) to the Next / Previous
Display (the `next-display` shortcuts for each window), and Move (All)
to Agent Screen ▸ (as the window menu's; a workspace heading's menu has
Move All Windows to Agent Screen ▸ too).

Twitcher (⌘Tab, ⌥Tab, ⌘\`, `twitch_step`; while ⌘ or ⌥ is held the
pointer chooses too: over a cell it is the one chosen, a click on it
chooses it and ends the switching, its release going nowhere:
`twitch_pointer`, `twitch_button`, `twitcher::cell_at`, the panel's
`cell_places` shared with drawing) shows each window it passes by
bringing it forward (a minimized one unminimized), but keeps the
stacking from when switching began (`Twitch::stacks`): passing on puts
the last one back in its place (minimized again if it was), so when ⌘ is
let go only the chosen window has moved. ⌥Tab has the apps on this
workspace; ⌘Tab (`Action::NextApp { all }`, `Twitch::all`) has a row for
each workspace with windows on it (`twitch_rows`, numbered at the left
when there are more than one), and so does ⌘\` for the focused app's
windows; each app's cell is its most recently used window. Tab walks
every cell in order of use (`AnvilState::recent`, kept by `note_recent`
in `tick`, not while switching), so ⌘Tab goes back and forth between the
last two apps wherever they are and ⌥Tab between the last two here (the
apps on the workspaces showing, every display's); ←/→
walk the chosen row and ↑/↓ go to the row above or below (its most
recent first). Twitcher's keys are shortcuts of the profile (Switch
Applications ⌘Tab, Alt+Tab on Windows, Ctrl+Tab on BeOS; Switch
Applications Here ⌥Tab; Switch Windows ⌘\`), and the keymap keeps the
modifiers of whichever began switching (`keymap::Switching`): letting go
of one of them chooses, its key again steps on (with Shift, back), and
other modifiers change nothing meanwhile. From the palette or an agent
(`run_shortcut`) one is a quick press and release. Deskbar highlights the application chosen in the
selection's colours, as Twitcher marks it (`style::selection_wash`; in
the lit workspace showing, the menu's selection, so it stands out on
it; `WindowInfo::chosen`, for its window; agents read `selected` on the
row's `Button`). Showing a window on another
workspace takes its display there, each workspace's stacking kept as it
was when first shown. Esc puts everything back, every display's
workspace and the focus too (`twitch_cancel`, `Twitch::home`). With keyboard hints on, every cell carries a hint
badge (`hints::badge_pixmap`, letters running across the rows,
`twitcher::Shown::letter`) and typing its letter while ⌘ (or ⌥) is held
chooses it (`twitch_pick`). The keys reach it as keymap actions
(`SwitchMove`, `SwitchPick`, `SwitchCancel`) only while switching. With
more than one display, the windows go by display as their Deskbars do
(`twitch_displays`, left to right, named as Preferences ▸ Displays names
them, "D1 Studio Display", so a cell is that display's workspace of its
row's number): each display
with windows is a column in an etched box, as a BeOS BBox (the kit's
`BBox` lines, `Palette::shade`), named across its top edge beside a
little screen that is lit on the display that copy of the panel is drawn
on (`twitcher::Display`, `Shown::displays`, `Entry::display`; the panel
is cached per display). Each app has a cell on every display it has
windows on, and ↑/↓ keep to the column of the display the chosen window
is on.

While switching, the chosen app's windows are spotlit (spotlight.rs, as
macOS doesn't and Windows' Alt+Tab peek does): once ⌘ has been held a
moment (`WAIT`, so a quick ⌘Tab flips without it) everything else fades
to black at 40%, the desktop too, under the chosen app's windows and
their dialogs (`AnvilState::spotlit`: children by `xdg_toplevel`
parent); ⌘\` spotlights the one window. `twitch_show` names them
(`spotlight::show`), `render::space_elements` draws the dim
(`spotlight::dim`, a solid redrawn only as its level changes) just
below the first window not named, so Deskbar, menus and Twitcher stay
above it. Choosing fades it out (`spotlight::end(true)`); Esc and the
lock end it at once.

## New windows (app_server shell/xdg.rs, shell/glow.rs)

A new window comes up focused and in front, whatever opened it (Olo's
apps, other programs, a portal's dialog, a program in the background):
`new_toplevel` gives it the keyboard as it is made, on the display and
workspace it opens on (the one showing), auto-tiled, zoomed in Focus or
grouped as its tab in front; wm-fuzz fails a step whose new window
doesn't have it. One an app opens from one of its own floating windows
(Preferences ▸ Sound's Try It, from the Sound panel) floats too, over
it, in Focus and Auto-Tiling (`new_toplevel`: the window that had the
focus is its app's and `floats`; it's floated as by hand): zoomed or
tiled, it was under the window it was opened from, floats being above
tiles (Ben, 2026-11-06). Windows coming back after an upgrade are the exception:
they come back as they were. Not an alert or a dialog of an app that
was started again (a new process: the old one is gone): it is placed
afresh, in the middle of the screen or over its parent, not where the
last one was, which may have been on a screen of another size
(`take_placement`); the same process reconnecting gets its back. A
window coming back finds its place by the key its app gave it first
(`layout::restore_key`: the xdg-toplevel-tag word `restore:KEY`, the
kit's `WindowOptions::restore_key`, saved as `Placement::key`), then by
its title, newest first, then (a terminal's title follows its
directory) the app's newest; never a place another key has. Titles
alone mixed up a group's tabs and an auto-tiled workspace's tiles: many
windows share one, and a Terminal window's comes only after it shows.
Terminal keys each window by a number of its own (`TermWin::key`,
`new_window_key`, kept in its saved state, whatever its tabs since);
give a key to any window that can have a twin in its app. A window asking to fill the screen (`fullscreen_request`) fills the
display it asks for, else the one it is on (`display_of`: most of it),
else the one the pointer is on; that display draws it alone
(`FullscreenSurface`), and the others leave it out
(`WindowMeta::fullscreen`) rather than draw its display-sized buffer
where it was, over their edge. Alone but for what's in front of it: a
window that comes up while it fills the display (a password dialog, a
new window: they come up in front) is drawn over it, its menus too, and
takes the clicks over itself (`shell::in_front_of_fullscreen`, the
windows above it in the space that show on that display, used by
`gather_elements`, `surface_under` and the click's focus; each tried
where it's drawn, its render location: its place less its geometry's
offset, a client-decorated window's shadow, as `Space::element_under`
does): drawn alone,
the full-screen window hid them, and a password dialog asked for while
Olo Setup filled the screen had the keyboard but was never seen (a
stock Fedora 45, 2026-11-07). A click on the full-screen window raises
it, as any window clicked (its own dialogs kept over it), so what was
over it goes behind until it's brought forward again (Deskbar, the
switcher, `activate`). With nothing in front the loop stops at
its first window (a display's gather, Setup full screen: 19.1 and 17.5
µs a frame before, 18.8 and 16.9 after). When that display's size in points
changes (a new scale, mode or rotation: `fixup_positions`), the window
is told the new size, so it goes on filling it. With Preferences ▸ Appearance ▸ "Glow new
windows for N seconds" (`Appearance::glow_new_windows`, off by default;
`glow_seconds`, 5) a new one (not one coming back) also glows a moment:
a soft halo in the look's `keyboard_navigation` blue, 12 points out from
its outline, its frame and its tab as one shape (stronger in a dark
look), whole until its last second and easing out over it
(`glow::strength`), drawn behind it (`WindowElement::render_elements`).
Its pieces are made at the window's size in device pixels and drawn one
to one (`render::exact`): a band across the top where the halo is the
distance to frame and tab together (so it goes up and round the tab),
a one-pixel strip of the falloff stretched down each side and along the
bottom, and the bottom corners round; made again only when the width,
the tab, the look or the scale changes, and let go once it has faded
(`WindowMeta::glow_strength`). Undecorated windows glow round their
geometry. Agents read `glowing` in `list_windows`.

## Workspaces, one set per display (app_server desktop.rs)

Every display has its own workspaces, `Settings::workspace_count` of
them, and switches between them on its own: `AnvilState::showing` keeps
each display's (by `display_key`, so a monitor unplugged and back shows
the one it did). A window belongs to a workspace of a display: while it
shows, the one its display shows (`workspace_of`, `shows`); hidden
(another workspace, minimized), the one it was left on
(`WindowMeta::workspace`) of the display it left (`WindowMeta::parked_on`,
which `display_of` reads; a display gone, the main one's).
`switch_workspace_on(output, n)` parks that display's windows and brings
back the ones left on its workspace `n`, the keyboard going to the front
one there (none if it has none, not another display's); the shortcuts,
`switch_workspace`, act on the active display (`active_output`).
`move_to_workspace` keeps a window on its display;
`move_to_display_workspace(w, to, n)` (`Request::MoveToWorkspace` with an
`output`: the window menu's and Deskbar's app menus' moves to another
display's workspace) and `move_workspace(from, n, to, m)` (Deskbar's
heading menu, `Request::MoveWorkspace`), both through `move_windows`,
move across displays as ⌃⌥⌘→ moves one (`move_to_output`, tiled or
zoomed as it was, hidden ones staying hidden, in front if that
workspace is showing there). Each display's desktop colour is its workspace's
(`desktop_color(output)`); auto-tiling's trees are per display and
workspace already (`(workspace, output)`), and every tick works on each
display's showing. The IPC: `{"type":"workspaces"}` answers the active
display's (`current`), the count, and each display's (`displays`:
output, current, and which is `active`); `WorkspaceChanged` says the
same, and is sent when another display becomes the active one too;
`{"type":"switch_workspace","index":n,"output":"DP-1"}` (output left
out: the active display); a window's `WindowInfo::workspace` is its
display's. Fewer workspaces asked for: the windows on those gone join
the last, and a display showing one goes to it.

## The overview (app_server overview.rs)

⌃↑ on a Mac, ⊞Tab on Windows (`Shortcut::Overview`, the palette, `{"type":"overview",
"show":true}`) shows every display's workspaces at once, macOS's Mission
Control with every workspace out together, where BeOS's Workspaces
applet has outlines too small to tell apart. Each display's usable area
(Deskbar left out, inside `MARGIN`) holds a pane per workspace of that
display with windows on it and the one it shows (`panes`: the grid that
makes them biggest, the last row centred), named above it as the
desktop's icons are ("D1 Workspace 2", `Pane::name`; bold for the one
showing, whose edge is the focus colour). In each
pane its windows are spread at one scale, no bigger than the pane's, in
rows in the order they stand (`spread`, a binary search for the biggest
scale that fits), lettered in reading order from `hints::LETTERS` with
the hints' badges. They are the windows themselves, scaled
(`constrain_as_render_elements`), so they go on drawing: those on other
workspaces are unmapped, and get frame callbacks from
`overview_frames`. Windows showing glide from where they are (`Cell::from`,
eased) and back on leaving; the rest, the panes and the letters fade.
The window chosen (under the pointer, else picked with the arrows, at
first the focused one) is ringed in the focus colour with its title on a
tooltip's plate at its foot.

A click on a window, its letter or Return makes it the active window on
its workspace (`close_overview(Some)`, which `restore`s it); Esc, its
shortcut again or a click on nothing leaves everything as it was. Keys go to it before
the keymap (`overview_key`, swallowing their releases too), except its
own shortcut and modifiers; clicks on Deskbar and other layers over it
go through, and anything else that chooses a window or a workspace
meanwhile (Deskbar, an agent) makes it step aside next frame
(`overview_interrupted`), gliding back to the desktop as it is then.
While it is out, `render::space_elements` draws its elements in place of
the windows of the displays it shows, front first: letters, the chosen
one's ring and title, windows, pane names, edges and fills, then the
dim over the desktop (Deskbar and other top layers stay above).
`{"type":"overview"}` tells agents what it shows: each pane, and each
window's frame, workspace, letter and whether it is chosen. The bench
(`tools/dev/bench`) times its frames opening, walked and leaving.

## Keyboard profiles (olo_protocol::shortcuts and app_keys, app_server keymap.rs, olo_kit_base::keys)

**Each keyboard's own** (Preferences ▸ Keyboard ▸ Keyboards): Alt and
the Super (Windows) key swapped, left and right, on the keyboards named
in `KeyboardSettings::swap_alt_super` (a PC keyboard, whose Alt is where
a Mac's ⌘ is, so with the Mac profile the key beside the space bar is ⌘;
an Apple keyboard on the same computer left as it is). app_server swaps
them as a key comes from such a device (`keyboard_key_to_action`, by
libinput's name for it; `keymap::swap_alt_super`), before the keymap;
synthetic keys never. `Request::Keyboards` lists the keyboards plugged
in (those with letter keys: not the power button's, the lid's or the
ThinkPad's extra buttons' pseudo-keyboards), and `InputDevicesChanged`
says when they come and go; a laptop's own ("AT Translated Set 2
keyboard") is shown as Built-in Keyboard, and one set up and unplugged
stays listed, to undo. A keyboard Olo knows (`olo_protocol::keyboards`,
by the name its driver gives it, as `mice` knows mice; only models seen)
gets a box of its own above, with its picture, what it is (maker,
model, keypad, how it connects) and what of it Olo can't use: the Magic
Keyboard with Touch ID's sensor (A2520) is sealed to the Mac it's
paired with (`TouchId::SealedToAMac`; TODO's "Keyboards we know" has
how that was found). An Apple Magic Keyboard (2021 or 2024, `fn_row`)
has its own there too: "Use F1, F2, etc. as standard function keys"
(`KeyboardSettings::fkeys_first`, by its name: app_server turns its top
row round, `apple_keys::fkeys_first`, from hid-apple's table, so it
needs no root and no module parameter, which is the whole machine's)
and "Press 🌐 alone to show emoji" (`globe_emoji`: KEY_FN pressed and
let go alone, quickly). Its row's features: brightness and volume as
any keyboard's, Mission Control (KEY_SCALE) the overview, Spotlight
(KEY_SEARCH) the palette, dictation (KEY_MICMUTE) the microphone, and
Do Not Disturb, which hid-apple sends as KEY_SLEEP: logind would
suspend the computer on it (the keyboard is tagged `power-switch`), so
app_server holds logind's `handle-suspend-key` for the session and
makes a Magic Keyboard's Do Not Disturb (by hand, as Deskbar's menu,
a moon on screen), any other keyboard's sleep key suspending as logind
would have. `OLO_KEYBOARDS_DEMO=1` lists the layout lint's
made-up keyboards (a laptop's and the A2520), to look at headless;
`ui_lint_keyboards_tab` lints the tab with them.

Every shortcut, app_server's and the apps', and how keys behave, are one
keyboard profile (`KeyboardProfile`, in `KeyboardSettings::profile`),
chosen in Preferences ▸ Keyboard ▸ Shortcuts: Mac (the default), Windows
or BeOS, each as true to its platform as it goes, with Olo's extras set
the way that platform sets keys:

* **Mac**: ⌘ is the command key; Rectangle's tiling (⌃⌥ arrows halves,
  ⌃⌥U/I/J/K quarters, ⌃⌥D/F/G thirds, ⌃⌥E/T two thirds, ⌃⌥↩ zoom, ⌃⌥⌫
  back to its size, ⌃⌥C center, ⌃⌥⌘←/→ displays), so ⇧⌘ arrows select
  text; Mission Control's spaces (⌃←/→, ⌃1…9, ⌃↑ the overview); ⌘Tab,
  ⌥Tab, ⌘\`; ⌘M hides a window, ⌘H the app; ⌘Space the palette, ⌥⌘Space
  Find, ⌃⌘Q lock, ⇧⌘3/4/5 screenshots, ⌥⌘⎋ Activity Monitor, ⇧⌘/ the
  shortcuts sheet; Finder's ⌘↑, ⌘⌫, ⇧⌘N in Tracker.
* **Windows**: Ctrl is the command key and nothing is translated; Snap
  (⊞←/→, ⊞↑ zoom, ⊞↓ back or hide, ⊞⇧←/→ displays), Olo's extra tiles on
  ⊞Alt letters; Task View's desktops (⊞Ctrl←/→, ⊞Tab the overview);
  Alt+Tab, ⊞D, ⊞L, ⊞S and a tap of ⊞ for the palette, ⊞⇧S / Print
  screenshots, Ctrl+Shift+Esc Activity Monitor, Alt+F4 closes any window;
  Explorer's F2, Delete, Alt+Enter, Alt+↑; Ctrl+Y redo, F3 find again.
* **BeOS** (as Haiku keeps it): the left Alt is the command key, the
  Super key is Option (Alt for apps), Ctrl stays Ctrl; Ctrl+Alt for
  windows (Z zoom, M minimize, H hide the app, B send to back), Ctrl+Opt
  for Olo's tiles, Alt+F1… workspaces (with Shift, taking the window),
  Ctrl+Alt+←/→, Ctrl+Tab Twitcher, Opt+Tab a window's tabs, Ctrl+Alt+Del
  Team Monitor (Activity Monitor), Print the Screenshot app; Alt arrows
  are Ctrl arrows (a word, the document), as on BeOS.

Keys are written "Ctrl+Alt+Shift+Cmd+Left" (`Chord`) and are always the
physical ones: Cmd is the Super key (⌘, ⊞, BeOS's Option), Alt is Alt.
`Chord::label(platform)` writes one as the platform does ("⌃⌥←",
"Win+Ctrl+←", "Ctrl+Opt+←"). Two tables give every shortcut its keys on
each platform: `shortcuts::SHORTCUTS` app_server's (`ShortcutInfo::keys`,
Mac, Windows, BeOS), `app_keys::APP_KEYS` the apps' menu commands, by
command (every app's: `app.copy`, `app.redo`…; one app's: `tracker.trash`,
`terminal.new-tab`…, which come first). A profile keeps only changes
(`KeyboardProfile::shortcuts`, by id; an empty list turns one off).
`shortcuts::entries` is both tables; `shortcuts::assign` gives one a
chord and takes it from any other it clashes with (app_server's from
every app, an app's from its own and every app's among themselves), and
the `defaults_never_clash` and `app_keys_never_clash` tests keep the
tables clean on every platform. The profile also has the command key
(`CommandKey`: Command, Control, Alt), Caps Lock as Ctrl, and a tap of
the Super key for the palette (`palette_on_tap`); keyboard hints and the
double-press speed are everyone's, in `KeyboardSettings`.

A platform's profile changed by hand becomes "Mac (Customized)"
(`KeyboardProfile::changed`, back to "Mac" when it's as it comes again)
and is kept among the saved ones (`KeyboardSettings::profiles`,
`profile_changed`), so picking another loses nothing; Save As… keeps one
under a name of its own, Delete forgets one (`choices`, `pick_profile`).
It all lives in app_server's settings, so Backup takes it along. Settings
from before profiles (`command_key`, `caps_lock_control`,
`find_on_command_tap` and `Settings::shortcuts`) are read once into a
Mac profile (`Settings::migrate`, on load and on every `set_settings`, a
restored backup included).

app_server (`keymap.rs`) takes the profile's command key (`Options`,
`command_key`): Super on a Mac, the left Alt on BeOS, which reach apps as
the right Ctrl; with Windows' keys nothing is changed. It looks up the
profile's bindings first; Twitcher's keep the switcher up while their
modifiers are held. A Mac's ⌘ arrows and ⌘⌫ are the line's and the
document's keys for programs that aren't Olo's; Olo's apps get them as
they are, so Tracker's ⌘↑ and ⌘⌫ are Open Parent and Move to Trash, and
the kit makes them the line's keys in a focused text field and the
editor (`keys::mac_command`: a Mac profile with its command key, the
right Ctrl held as the action comes; then a word's move is the line's
start or end, ⇧ selecting to it, and ⌘⌫ deletes to the line's start,
`Cmd::DeleteToLineStart` in the editor; ⌘↑ and ⌘↓ are the document's
in the editor, and left to the window's menu in a field). (The egui
kit's `mac_text_keys` did this; it went with egui on 2026-11-02, and
until 2026-11-03 ⌘← moved a word in Olo's own text.)

The kit (`keyboard.rs`) follows the profile from app_server's settings
(`keyboard::profile`, `platform`, `command_key`). Every menu bar and
context menu gets its keys from it (`keyboard::resolve`, in
`prepare_menus`): an item whose command the app has keys for in
`app_keys` gets them, every app's only where the app gave the item keys
(so a bare item doesn't pick up another's), and shows them as the
platform writes them; the app-side chord (`menu::Shortcut`: the command
key, Alt, Shift, or a key of its own such as F3 or Delete) is what is
matched (`keyboard::app_side`). A debug build warns once about a menu
item with keys the table doesn't list ("menu keys not in
olo_protocol::app_keys"): add it there. Keys an app takes outside its
menus ask the profile too (`keyboard::pressed` / `consume` by command:
StyledEdit's Undo and Redo, Tracker's desktop and Find results, Disk
Usage), and Terminal's own are `keyboard::is_own_shortcut`.

The shortcuts sheet (`Shortcut::ShortcutHelp`: ⇧⌘/, Win+Shift+/,
Alt+Shift+/) is the palette's (`olo-palette --shortcuts`, sheet.rs): the
front app's menu keys (`commands::menus`, as it shows them), text
editing's for the platform, then app_server's by group, as cards in
columns (`masonry`), in a scroll view when they don't all fit; Esc, a
click outside or the keys again put it away. Its text is in the
AccessKit tree (`style::key_combo` describes keys as labels do).

## Desktop shortcuts and tiling (olo_protocol::shortcuts, app_server keymap.rs, shell/tiling.rs)

app_server's own shortcuts are one table, `olo_protocol::shortcuts::SHORTCUTS`
(see "Keyboard profiles" for their keys on each platform): each a
`Shortcut` (tile the focused window into a half, quarter or third, zoom,
zoom along one axis, back to its own size, center, to the next display,
next / previous tab, hide it or its app, send it to back, tile or cascade
all windows on the display, show the desktop, workspaces, Twitcher, the
overview, the palette, Find, the shortcuts sheet, Activity Monitor,
lock, caffeinated, screenshots), a group, a title and its keys on each
platform. To add one: a `Shortcut` variant, a row in `SHORTCUTS` with
keys natural to each platform, its case in input_handler's
`run_shortcut`.

Preferences ▸ Keyboard ▸ Shortcuts lists them and every app's; clicking one sends
`Request::RecordShortcut`, and app_server takes the next key with its
physical modifiers (only it can tell ⌘ from Ctrl) and answers with
`Event::ShortcutRecorded` (Esc cancels, ⌫ clears; it gives up after 30 s).

Tiling (`shell/tiling.rs`) works on the display's usable area (no
Deskbar or panels) less `Settings::tiling.margin` (`tile_area`), with
`tiling.gap` between windows (`part`: each inner edge gives up half).
Every arrangement keeps to it: tiling, Tile All and Smart Tile, zooming
(`zoom_area`) and zooming along one axis, Center, Cascade and Fan Out.
When the margin or gap changes (Preferences ▸ Windows),
`tiling_space_changed` refits every tiled and zoomed window in place
(`refit`) without changing which is in front: a zoom, an unzoom or any
other move of a mapped window relocates it (`relocate_element`), since
`map_element` raises and activates. Windows of their own size only
have to stay on the display. A tiled or zoomed window remembers its own frame
(`PreZoom`), which Restore puts back, and which part it was tiled into,
so it is re-tiled when its display changes. Unzooming goes back to where
it was zoomed from: its tile, a place Tile All gave it (`ZoomedFrom`), or
else its own frame. Moving it by hand forgets the tile (`forget_tile`).
Resizing it by an edge moves its tiled neighbours with it (`TileDrag`,
driven by the resize grabs): at the drag's start, `links_for` finds the
other tiled windows on the display with a side on that edge's line
(across the gap, or in line), which follow it live. Each step of the
drag is a round (`shell::rounds`): the dragged window and every
neighbour are sent their new sizes at once (`tile_drag_next`, from
where the pointer has got to), and what each draws for it waits, a
commit blocker (`dmabuf_check::Gate`, in the pre-commit hook:
`round_commit`), till every one has drawn it at the size asked, its
GPU fence says the drawing is done and its buffer, if new, has been
checked (`round_fenced` counts what each waits for, `round_ready` each
done; a buffer's check gate tells the round when it opens,
`dmabuf_check::Gate::in_round`, `gate_opened`: without it, a window
drawing into new buffers as it grew showed a round or two behind, its
frames queued behind the check), so they show in the same frame: no tile over another and no gap between
them at any moment. The next round goes as one shows, so the tiles move
at the rate the slowest of them draws, the display's when they keep up
(at 120 Hz a round takes 7 ms, one a frame); one that hasn't drawn in
`LIMIT` (50 ms) shows when it does and the rest don't wait. Letting go
is a round too (`round_last`: the dragged window's final configure,
which `end_resize` leaves to it, with the neighbours' last sizes and
whatever auto-tiling lays out otherwise as the drag ends, which `follow`
puts in it while it's made: `round_collecting`; a tile that only moves
then, drawn at its size already, moves as the round shows,
`round_move`, not a frame or more before the rest). A round that runs
out of time names the windows that didn't keep up in the log;
`RUST_LOG=info,olo::rounds=debug` (a `tsession start` takes it) traces
every round: what each window was asked, each commit (in which round,
a new buffer still being checked or not), fences, and where each window
landed.
Without rounds each window was paced on its own and moved when it drew:
neighbours trailed the edge by up to 600 px or ran over the dragged
window, and some apps answer a configure with a frame of the size
before (the kit's GPUI may: its window geometry is the new size at
once, but the frame drawn came from the old one), so a member counts as
drawn only by a buffer of the size asked (the viewport's source at the
window's scale, for the kit's apps, which draw into part of a bigger
buffer). Each window is moved only when it commits
(`tile_follower_drew`: the sides that don't move stay put, whatever size
it drew), and keeps catching up after the drag (`Follower`). A neighbour
is resized as much as the window dragged, and told so (xdg's
`Resizing`, set in `ask_to_follow` while the drag lasts, cleared in its
last round): the kit's apps then keep room to grow (GPUI's
`update_drawable_size_within`), where before each made a new swapchain
at every step, four new buffers of GPU memory each checked and freed by
app_server, and its round waited for the checks (a 150-step auto-fuzz
run: 8,324 new buffers before, 2,188 after). A
neighbour linked on two sides (a corner drag) moves both. Auto-tiling's
layout leaves every window the drag moves to it (`TileDrag::moves`):
the tree has their tiles where they were until the drag ends, and laid
out each turn it asked them back there, against the drag. `timings`
counts rounds as phases (`drag round`, and `drag round (out of time)`).
The drag stops where
one would get smaller than its app's minimum or `MIN_TILE`, the window
dragged included (`TileDrag::min`, its group's: a tab behind it may
take more than the front one's app); auto-tiled, also where its tree
would lay the tiles out otherwise than the drag has them
(`tile_drag_limit` asks `auto_keeps`, `autotile::lays_out_as`: the
tree's splits taken from the drag's frames, laid out with every
window's least size, must put them back just there; `furthest` finds
how far it may go). So what a drag shows is what stays as it's let go:
before, the drag counted only the least sizes of the windows on its
edge, and a split's side holding more (a row of two beside a column, a
group's tab) or sides whose least sizes don't all fit (laid out at
their shares, whatever the ratio) moved the tiles as it was let go, out
of step. Edges a drag moved by a pixel or two count too (`take_ratios`'
`still`, a pixel and a half, `still_in`: it was 0.3% of the area, 5 px
on a big display, and a small drag snapped back). At the end
each keeps its new part (`TilePart`, fractions that `part` turns back
into the same frame: `fraction_of`), which `retile` uses from then on (a
tiling command starts afresh; Tile All and Tile App tile each window as a
`Tile::Cell`, its grid cell as its part, so they link the same way;
rolled-up windows don't get a cell each but share the last one
(`tile_grid`), each the whole cell with their tabs side by side along its
top, a row lower when a row is full (`fan_in`), so unrolling one fills
the cell). A tiled window with no tiled neighbours
on that line is being sized away from its tile, and forgets it. Windows
not tiled by a command are never pushed. Tiles and parts are kept across
a restart in the layout (`Placement::tile`, `part`). Moving screens in
the arrangement takes their windows along (`fixup_positions`); each
window goes with the screen it was on as the screens were last arranged
(`Arranged`), not by where it is now, so a screen that got smaller (a new
scale or mode) keeps its windows (one past its new edge comes in from
it) rather than handing them to the screen beside. When displays come
or go (`follow_main_output` → `rehome_windows`), windows left on none go to
the main display, cascaded and shrunk to fit, the rest are re-tiled,
re-zoomed or fitted where they are, and the pointer is brought back; a
minimized window is fitted when it comes back.

A display unplugged (`display_leaving`, before its output leaves the
space: udev's `connector_disconnected`, a display made a mirror, a test
screen) sends its windows to the main display of the rest, each onto
the workspace of the same number, tiled or zoomed as it was
(`move_windows`, so each remembers its round trip), hidden ones hidden,
and keeps its tiling trees aside by its monitor's key
(`AutoTiler::leave`). Each window remembers that monitor and workspace
(`WindowMeta::away`). Plugged in again (`displays_changed` →
`windows_back_home`), the display gets its trees back
(`AutoTiler::come_back`) and its windows return onto their workspaces
exactly as they left (`move_to_output` with `home`: frame, tile and its
size, zoom), whatever Olo did with them meanwhile; one the person moved
on purpose meanwhile stays where they put it. Test sessions unplug and
plug a screen with `{"type":"plug_display","name":"HEADLESS-2",
"plugged":false}` (its `wl_output` taken away, then offered again).

Fan Out (`fan_out`, ⌃⌘/) stacks the display's windows top-left with their
tabs side by side: in the order of their tabs' x (front-most first on a
tie), each starts at the last one's tab end, is narrowed so it doesn't
reach under the Deskbar but not below a quarter of the display (nor its
app's minimum), and past that is pinned as far right as it fits with its
tab slid along it (`tab_offset`, kept across a restart in the layout),
so the tabs overlap only then. Zoom All (`zoom_all`, ⌃⌥⌘↩) zooms every resizable window on
the display, or unzooms them all if they all are. Whether a window is
zoomed is in `WindowInfo::zoomed`, for agents, and one zoomed only one
way (Zoom Vertically, Zoom Horizontally) says which in
`WindowInfo::zoomed_along` (`"vertical"`, `"horizontal"`): Zoom
Vertically on a zoomed window leaves it zoomed horizontally. Window to Workspace 1–9
(⌃⇧⌘1–9) moves the focused window there and follows it. Cascade Others and Fan Out Others (⌃⇧⌘\,
⌃⇧⌘/: `keep`) arrange all but the focused window (or the one the
palette or Deskbar's menu names), which stays where it is, in front and
focused (`windows_to_arrange`, `bring_back`). Every
arranging command (Tile All, Tile App, Cascade, Fan Out) first calls
`remember_arrangement`, which keeps each window's frame, tab offset, tile
and part, and whether it was zoomed (fully or along one axis, and what
unzooming would put back) (`Placed`, in `AnvilState::arranged`); Undo
Window Arrangement (`undo_arrangement`, ⌃⌘Z) puts them back, a zoomed
window zoomed again, and remembers what it replaced, so undoing again
redoes. Agents run both with
`{"type":"run_shortcut","id":"fan-out"}` / `"undo-arrangement"`.

Windowing modes (`shell/modes.rs`): each workspace of each display is
Standard (windows stay where they are put; the default), Auto-Tiling or
Focus. The mode is chosen from its Deskbar heading (or the Olo menu's Window
Arrangement), ⌃⌥⌘T (`auto-tiling`) and ⌃⌥⌘O (`focus-mode`), which turn
theirs on or back to Standard, the palette, or
`{"type":"set_windowing_mode","mode":"focus"}` (`standard`,
`auto_tiling`; `workspace` and `output` optional;
`{"type":"windowing_mode"}` reads it, each display's on the workspace it
shows unless one is given). It is per workspace of each display
(`TilingSettings::modes`, `mode` / `set_mode`, which keep only what
differs from `default_mode`, Preferences ▸ Windows' Default mode; a
choice kept for a workspace on every display, as older versions kept
them, still applies to each display without its own).
Settings from before modes kept auto-tiling's ons and offs
(`TilingSettings::auto`), which `upgrade` reads into `modes` on load.
Each change shows its OSD (`osd::Indicator::Mode`: the mode's icon, name
and `WindowingMode::detail`; `OLO_OSD_SHEET=/var/tmp/osd.png cargo test
-p app_server osd_sheet` draws every indicator at every scale to look
at). The arranging commands (tiling, Tile All, Cascade, Fan Out, Smart
Tile) leave Focus for Standard first (`arranging`), saying so.

Focus shows one window at a time on its display, zoomed: the front one
of those that would be tiles (not floating, placed, not rolled up). Activating another (Deskbar, ⌘Tab,
Twitcher, the overview, a click on a tab) raises it, so it is the one
that shows. Every one of them is zoomed (`zoom_window`, so leaving Focus
puts each back where it was; `focus_tick`), a new one opens zoomed
(`focus_claim`, at its first commit, and stays so while it is placed:
`focus_tick` leaves a window that isn't placed yet alone), and one moved
or sized by hand zooms again where it is let go. An app's windows are
grouped there (`group_key`), whether or not the app groups them
elsewhere, but one taken out of its group (Remove from Group: `Alone`)
stays out and shows on its own, the app's others not its tabs. A group
zooms as one
(its front window's arrangement is its others', `share_arrangement`):
leaving Focus (`leave_focus`, for that display's windows only: another
may still be in Focus) puts back every window of a group Focus zoomed,
whichever of them is in front by then, or the one in front would lead
the rest back into its zoom. An app's windows are always one group
there (`group_key`), so its others are tabs along the top. The rest are
covered (`WindowMeta::covered`, `mark_covered`, from the stacking order
just before drawing): not drawn and not under the pointer, but still in
the space at the same place and size, so showing one is only raising it,
and Deskbar, Twitcher and the overview have them as ever. Windows that
float (`floats`, as for auto-tiling) show over the one in front
(`keep_floating_over_focus`); a dialog (a window with a parent) is
covered with its parent, and an app's own panel (no parent: StyledEdit's
Find) while all its app's windows are, as BeOS's floating app feel did;
an app of only panels (DeskCalc) always shows, and so does an alert that
names no window (`is_parentless_alert`: Passwords' passkey question
while Chrome is in front, an app's "Save changes?" asked from Deskbar),
which asks the person something whichever window shows. `WindowInfo::covered`
says which are covered, for agents, and `WindowInfo::stack` (in
`list_windows`, for windows on screen) where each is in the stacking
order, 0 the front: a window is in front on its display when none there
has a smaller one.

Auto-tiling (`shell/autotile.rs`; its module doc is the design) is the
Auto-Tiling mode; `{"type":"set_auto_tiling","on":true}` and
`{"type":"auto_tiling"}` still work, as the mode. While it is on, each display of the workspace has a
tree of splits (`autotile::Node`, in `AutoTiler::trees` by workspace and
connector), and every window there is a leaf of it, a window group one
leaf. A new window fills an empty place if there is one, else splits
the tile that last had the focus (if both fit; else the biggest) along
its longer side (`auto_insert`, `cut_for`, as bspwm and Hyprland's
dwindle do) and gets its tile at its
first commit (`auto_claim`), so it opens at that size rather than
drawing another first; a closed one's room goes to its sibling.
Where the tiles then can't give every window its least size (no tile
had room for two, or a window's least size grew), `autofit::refit`
arranges the tree again so they all fit, if any arrangement does: the
fewest splits turned the other way; else, from every group of tiles'
shapes (the least width and height each grouping can be laid out in, a
slicing floorplan's shape curve, over any grouping up to
`autofit::SUBSETS` tiles and runs of their order beyond), a tree built
from the top, across the longer side and as even as fits; else the one
that overlaps least, when that's less than now. It runs only where the
tree overflows, in a few milliseconds at most. `layout`
is O(windows), keeps each app's minimum size and runs only when
something changes: `auto_tick` compares a glance at the windows and the
displays' room (`AutoTiler::glance`) with the last, so a turn of the
loop with nothing new (the pointer moving) costs next to nothing. Moving
the windows goes through `set_frame` and, for a window already drawn,
the tile-drag `follow` path, so nothing waits on a client and nothing
jumps ahead of its picture. The tiling commands work on the tree
(`tiled_into`: the window takes its half, quarter or third and the rest
tile in what is left), as do Zoom Vertically / Horizontally (`along`: a
full column or row beside the rest, and back), Tile All (`balanced`),
dragging a tile by its tab (`drop_at`: onto the middle of another swaps
them, onto its edge puts it beside) and dragging an edge (`take_ratios`:
the split stays just where it is let go, with no grid to snap to, and
the other windows keep the sizes given them as windows come and go, a
new one halving only the tile it goes beside). Zoom zooms over the tiles and
unzooms back into its tile. ⌃⌘Z undoes the last change to a tree, or
turning it on (`auto_undo`, before `undo_arrangement`). Windows that
float instead (`floats`): those with a parent, modal dialogs, a
fixed-size window (one size only, across or down) or one whose largest
size can't fill its display's tiles (Electron's i32::MAX, like
others' 0, is no largest size: it tiles), one tagged `floating`
(xdg-toplevel-tag; the kit's `WindowOptions::floating()`, which
DeskCalc, Preferences, Tracker's Info and status windows and the like
set, and its `dialog()` and alerts, which it tags so too), and for other
programs portal dialogs (`floating_kind`) and picture in picture. An
xdg-dialog alone doesn't make a window float (nor keep it out of its
app's tab group, `groups::is_dialog`): GTK 4 gives every toplevel one,
its main windows too, which then floated over the tiles and never
grouped. Floating windows stay in front of the tiles, and of the
window Focus shows (`keep_floating_above`, `keep_floating_over_focus`),
and among themselves stack as windows do: the one clicked or activated
comes to the front of them, one that opens starts at the very top, and
a tile activated takes the keyboard with the floats still over it. Both
run wherever a window is raised (`focus_window`) and as the tiles are
laid out; when one is under a tile they raise every float, bottom
first, so their order among themselves stays (raising only those
under a tile put one over the float brought forward last, again and
again: Ben's "one of them keeps itself on top and i can't go to the
other floating one"). Anyone can float or tile a window by hand: the
window menu's Float Over the Tiles (only where auto-tiling is on),
⌃⌥⌘F (`float-window`), or `{"type":"set_floating","id":ID,"floating":true}`
(`null` leaves it to app_server again); `WindowInfo` says `tiled`,
`floating` and `float` (the hand's choice). Some windows float whatever
is asked (`floats_always`, in `WindowInfo` too): a dialog that names its
window, an app's alert (over all its windows), picture in picture. Each
belongs over another window, and a tile can't be over the floats: an
alert tiled by hand stayed over its app's floating windows as their
child (`keep_children_above`), a tile over floats, which the next pass
put under them again, and so on (the auto fuzzer found it). For those
`set_floating` does nothing, and the window menu shows Float Over the
Tiles checked and dimmed. The choice is its group's:
a group is one tile, or floats, as its front window does, so every
window in it is given it (`set_floating`), and whichever comes back in
front after a restart says the same (a floating group came back tiled).
A window that joins a group later (one the app opens, grouped by its
app) takes the group's choice (`sync_groups`): it was a tile at the
floating group's frame, over the tiles beside.
A window placed (`place`) onto an auto-tiled display from another
floats there where it was put, its group with it. A window that leaves for a
while (hidden, to another workspace) leaves a `Kept` place that takes no
room and is its again when it comes back; a closed one leaves one for
20 s (`CLOSED_KEEP`), which a window of the same app takes, so an app
re-executing after an upgrade gets its tiles back. An in-place restart
keeps the trees themselves: app_server writes them beside the layout
(`olo-layout-<display>.tiles.json`, `autotile::SavedTree`), each tile
naming the saved places of its window and its tabs on that workspace
(`Placement::slot`; a closed window's by the app and title auto-tiling
knew it by, `auto_known`, or one like it already back), each split at
the ratio it is laid out at (`as_laid_out`: a window's least size can
hold an edge off where it was dropped; while windows are closing for an
upgrade, as the tree was last laid out with them all, `laid`). The next
app_server starts with those trees, every tile's room held for 20 s
(`Who::Returning`), and a window put back from a place takes the tile
that names it (`auto_returned`, by `WindowMeta::slot`), on any
workspace, in whatever order the apps come back. A place that took no
room (an away window's, a closed one's) is saved as such
(`SavedNode::Away`, `Like::holds`) and comes back taking none: held,
it was a hole where the tiles had met, for 20 s after every upgrade,
then closed with the tiles jumping into it. A window coming back to a
display showing its tree claims its tile at its first commit
(`auto_claim_returning`: the leaf naming its slot, or it, or one like
it), so its first configure asks its tile's size as it is now and it is
shown there (`restore_placement` keeps it there, its saved frame and
tile unused): put back at its saved frame first, it was a frame over
its neighbour after the tiles had moved since the restart (a drag,
others coming back). When several of a group's windows, each with a
tile, go into the group at once (groups coming back together), the
group keeps the first of their tiles and the others close (each given
to the group's front, it was laid out in two tiles). A window coming
back whose saved place is among those another's kept tile was held for
(its group's: `kept_for`) gets no tile of its own while windows are
still coming back (`RESTORING`), nor one about to rejoin its group
(`joins_a_group`): its group forms in that tile (as a tab of a window
not shown yet, it split a neighbour's tile for a few frames). The check of
windows back after a restart (`check_restored`) leaves alone a window
behind a tiled one in its group, as it does a tile (moved, it came to
its group's front, out of the tile). Without saved trees
(from an app_server before them) they are made again from the windows'
saved parts (`auto_adopt`, `WindowMeta::restored_part`). An edge drag's
splits are set where the windows' least sizes let them be
(`auto_drag_ended`). Each window's least size counts a whole gap
(`auto_mins`), but a tile at the area's edge gives up only half of one
there (`tiling::part`): a split's limits take that back (`Inset`, in
`layout` and `as_laid_out`), else a tile dragged to its least size
beside the edge was laid out 4 px wider as the drag was let go, and its
neighbours jumped. A hand's choice to float or tile a window
(`Placement::floating`) comes back too.

Picture in picture (`shell::pip`) is a video, or a page, popped out of
a browser's tab: Chrome's video is titled "Picture in picture" (with
no app id), a page (documentPictureInPicture, Meet's call as its tab is
left) has its opener's title and, unlike Chrome's other windows, a
largest size, so `is_pip` knows it by that, decided once at its first
commit. As on macOS it opens in the bottom right corner of its display
at the size it asks for, stays over every other window
(`keep_pips_above`, from `keep_children_above`) and on each workspace
of its display, and is never in a group, a tile, Focus's zoom or
cover, or a remembered place (its title would put another window
there). Opening one, or its activation, leaves the keyboard where it
was, and a front window that goes hands it on past one. Meet's call
used to join Chrome's group in Focus and zoom over the tab a link had
just opened in. A launch's activation token (`ActivationToken`, which
olo-chrome hands Chrome with a link) covers its client's requests for
`LAUNCH_GRANT` (2 s): Chrome spends the one it was given on whichever
window it brings forward first and asks again for the next.

Roll Up (`toggle_rolled`, ⌃⌘M, the tab's menu, the palette, or a
double-click on the tab, which used to hide the window) leaves just the
window's tab: `Decoration::rolled` makes the element's bbox the tab, draws
only the tab, and passes the pointer through everywhere else. It is in
`WindowInfo::rolled` and kept across a reload (`layout::Placement`).
Agents roll with `{"type":"run_shortcut","id":"roll-up","window":ID}`.

Zoom Vertically / Zoom Horizontally (`toggle_zoom_along`, ⌃⌥⇧↑ or ↓ /
⌃⌥⇧← or → on a Mac, the window menu, or a double-click on the frame's top or
bottom edge / left or right edge) zoom the window along one axis only:
the whole height (width) of the tiling area, the other axis as it is,
pulled in and narrowed only if it doesn't fit (`zoom_along`). The window
keeps where it came from (`Along`: a `ZoomedAlong` with its frame, its
tile and part, and whether it had a `PreZoom` already), and the same
command again unzooms that axis only (`unzoom_along`): back into its
tile if it had one, else to its old height (width), keeping whatever was
done across meanwhile; a window untouched since it zoomed gets its whole
old frame back, even where fitting pulled it in (`unzoomed_along`).
Zooming the other axis too is Zoom (and Zoom's unzoom goes back to the
one-axis zoom, `ZoomedFrom`); unzooming one axis of a full zoom leaves
the other zoomed. The zoomed axis follows its display (`refit`,
`move_to_output`, `rehome_windows`). A window sent to another display
takes its own frame and what unzooming puts back along, at the same
relative places; sent straight back, untouched, it has exactly what it
had again, however the two displays differ in size (`Travelled`: the
frames it left and arrived with). Any tiling, arranging or moving or
sizing by hand forgets it (`set_tiled`). As with Zoom, a reload keeps
the geometry only. Agents:
`{"type":"run_shortcut","id":"zoom-vertical","window":ID}`. Every turn
`sync_held_sides` tells each window which sides app_server holds, as
xdg's tiled states (all four for a tile, auto-tiling's too; top and
bottom zoomed vertically; left and right horizontally; `held_size`), so
an app sizing itself (Terminal keeping its columns as its text size
changes) asks only for what it may have: the kit keeps the held size
(`Kit` `held`) rather than flicker to one that is put back.

A window that comes back (from another workspace, from being hidden) is
left exactly where it was unless the displays changed while it was away
(`welcome_back`: then it is rehomed, having first moved as far as its
own display did, `display_moved`); a tiled or zoomed one is only
refitted to its room, in case the Deskbar moved. It comes back on the
display it was parked on, whatever its frame says (`unpark`: brought
into that display's area if its frame would show it on another, or on
none): an Undo putting a window back where it was before the displays
changed, which they did again, once left it off every display, and when
its workspace next showed it appeared on the other display, which
doesn't show that workspace, and stayed there (wm-fuzz seed 5,
Twitcher's ⌘`). Whatever a window
remembers (its own frame for Restore, what unzooming puts back) moves
with its display too (`shift_remembered`, from `fixup_positions`), as
does what Undo Window Arrangement puts back (`arranged_among`), and is
put back in reach whatever happened meanwhile (`reachable`). A display
moved in Preferences leaves the others where they are (`keep_neighbours`
pins them). `place` (agents) puts a window where asked but in reach: its
top edge on a display, enough of it to take hold of (`into_reach`).
`refit` leaves a window alone when it fits and is in reach (`in_reach`:
the top of its tab or frame inside the area, at least 48 px of it
across), even hanging off an edge, as people leave windows.

Next Tab / Previous Tab (⌃⇥ / ⌃⇧⇥ by default) are keys, not actions:
the keymap types Ctrl+Tab / Ctrl+Shift+Tab into the focused window
(`stand_for`), which Chrome, Firefox, GTK, Qt and the kit all take for
the next and previous tab, and so does a terminal (whose ⌘ is
Ctrl+Shift, but physical Ctrl stays Ctrl). Set to other keys, with ⌥
say, Ctrl goes down before ⌥ is let go of, and ⌥ stays up for the app
until another key needs it (`restore_lifted`), so the app never sees a
lone tap of ⌥ (Firefox would show its menu bar). A Next Tab set to ⌥Tab
takes it from Twitcher. From the palette
or an agent (`{"type":"run_shortcut","id":"next-tab","window":ID}`),
app_server focuses the window and types the same keys. In the kit,
`widgets::tab_keys` takes them out of the input (+1 / −1, Ctrl+Page
Down / Up too): `TabView` steps through its tabs with it, the first one
in the window, and so does Terminal; anything else with tabs should too.

Window groups (shell/groups.rs): an app's top-level, decorated,
resizable windows on the same workspace of the same display share one
frame (`current_groups` keys them by app and display), with a tab
each laid side by side along the top (`tab_bar`: each tab as wide as its
title wants, or all an equal share if they don't fit, so tabs never
overlap; `Decoration::tab_width`, `tab_offset`). `sync_groups` runs every
`tick`: the front-most member leads, newcomers take its frame
(`join_group`), and the group is raised together so clicking a tab
brings that window to the front of the frame. The leader is the
front-most window that was already in the group (then one already on
screen, then the front-most), at the size it was asked for if it hasn't
drawn it yet, so a window that opens, comes back or arrives from
another workspace joins the group where it is instead of pulling it
along; the others take the leader's arrangement too (tile, zoom, the
frames Restore and unzoom go back to: `share_arrangement`). Such a
window, though in front, doesn't lead until it has drawn at the group's
size (`Groups::settling`, up to two seconds; asked again if its app drew
another size anyway, as Terminal fitting its grid can), so the size it
opened at never becomes the group's. Sizes wait only while a window's
edge is being dragged (a resize grab), not for any pointer grab: a
menu's lasts until the pointer next moves, and a held button is one. A window
that joined a group on arriving keeps the frame it came with
(`WindowMeta::joined`): sent on to a workspace where it is on its own,
untouched, it has that frame again, so a window taken to another
workspace and back comes back as it was. A group that comes apart
around it while it shows (the others closed or gone) leaves it the
group's frame as its own: it isn't put back in the one it came with
the next time it comes back (mode-fuzz found one jump after a Focus
round trip and a workspace switch). Only the
front window is drawn whole and takes input over its frame: the ones
behind it show just their tabs (`mark_behind`, before every frame,
`Decoration::behind`), so they never show around it while it is sized
smaller; a capture for an agent draws a window whole. Being behind, a
window gets frame callbacks once a second (Smithay throttles a surface
on no output), so its last buffer may be that old: one brought to the
front while its client waits to draw (`waiting_to_draw`) is told to
draw at once and shows only its tab, the old front still drawn, until
it commits or `FRONTING` (100 ms) passes (`Decoration::fronting`): a
Terminal tab clicked showed what it had printed a second before, then
jumped to now. It waits once: one new to its group waits so too, till
it has been the front (`Decoration::fronted`), and a wait ended by its
drawing counts as that (`drew_while_fronting`): a commit asks for the
next frame, and taken for one still to come to the front, a busy window
was held back after every frame it drew, the old front showing for
seconds (a Terminal with an agent streaming, after an upgrade, when no
window has been the front; `top_turn`,
`a_window_brought_forward_waits_once`). Moving, resizing,
rolling up and tiling (one cell per group) act on the whole group; an
arranging command on a window behind brings it to the front first
(`front_of_group`). A
window leaving a group gets its own tab back and steps out by `CASCADE`
so it isn't hidden. Grouping is on for every app; the ones a person
turns off are `TilingSettings::ungrouped_apps` (app ids), toggled by
Group Windows in Deskbar's team menu and in Tracker's menu for an app
(`olo_ui::settings::groups_windows` / `set_groups_windows`). A group's
frame is never smaller than any of its windows can be (`least_frame`).
A window that joins a group it can't be as small as draws bigger only
on that side: `keep_on_screen` fits to the screen only the sides an app
chose itself, never one it took as asked, so a Tracker put in a Terminal
group sized by hand taller than its display is wider, as tall, and
doesn't pull the group back onto the screen. A window brought back into
its group (from hidden) likewise only grows it to that window's least
size, where it is. Tiled (by a tile shortcut), a group grown past its
tile for a window that needed it is its tile's size again once that
window has gone (taken out, dragged out, closed): `back_to_tile` in
`sync_groups`, by `tile_frame`, where `retile` would put it.

Only an app's own windows group by themselves, but any windows can be
grouped by hand (`WindowMeta::grouping`: `Auto`, `Alone` once taken out,
`With(key)` once put in one; `tab_order` places each tab). ⌘-dragging a
tab (shell/tabdrag.rs) works as Haiku's Stack & Tile does, with ⌘ for
its Option key: along its group's tabs it reorders them, the others
making room where it would go; pulled `TEAR` px above or below the tab
row (or `TEAR_SIDE` past its ends) the window comes out and follows the
pointer on its own (`Alone`, so it doesn't just join its app's group
again); held within `NEAR` px of another group's tabs, or a lone
window's, it folds into just its tab in the place it would take, and
that group's tabs and frame light up in the focus blue
(`Decoration::target`); let go there, it joins at that place
(`put_in_group`, showing only its tab for up to `DOCKING` while it draws
at the group's size). Back over its own group, it goes back in. A plain
drag still moves the whole group. The window menu (the palette's) has
Remove from Group. Dialogs, fixed-size panels and fullscreen windows
never group (`groupable`). Agents do the same with `GroupWindow { id,
with, index }` (into `with`'s group, or a new one with it), `UngroupWindow
{ id }` and `MoveTab { id, index }`; `WindowInfo` says `group` (a number
its members share), `tab` (its place), `groupable`, `output` (the
display it is on, or was on while hidden) and, in `Reply::Windows`,
`tab_frame` (where to click or ⌘-drag its tab).

A window moved by hand over another of its own app's windows' content
(`own_window_under`: the top window there, not covered, same client) is
drawn see-through (`WindowMeta::ghost`) and its app is told where, and
where it is let go (`window_drop.rs`: `{"window_over":…}` and
`{"window_dropped":…}` lines on the app's kit socket, from a worker
thread that keeps only the latest place, so the compositor never waits
on an app): a kit app's tab strip marks the place, and a window let go
on it becomes tabs there (`App::window_dropped`). A move a client asks
for with the press of another of its windows (a tab torn off a strip,
`olo_ui::begin_move`) starts from the pointer, not from that press; one
asked for before the window has shown (Vulkan's present commits its
first buffer on its own, after the request) waits till it is placed
(`WindowMeta::pending_move`). A window tagged `under-pointer` opens
with its tab under the pointer, alone (`place_under_pointer`).

Other programs arrange windows through two requests: `Arrangement`
answers the active display's connector, its tiling area (global logical
pixels) and its windows on this workspace, front first, with frames;
`Arrange { windows: [Placement { id, x, y, w, h }] }` puts the listed
ones in those fractions of the area (`arrange`), as Tile All does its
cells: undoable, and tiled as `Tile::Cell` so their edges move together.
Smart Tiling (see "AI features") is the first user.

## Keyboard hints (app_server hints.rs, Preferences ▸ Keyboard)

Opt-in (`KeyboardSettings::hints`). Pressing Control twice quickly
(either Control, or Caps Lock when it is one), with nothing else down or
typed in between, is the keymap's `Action::Hints`: the second press must
come within `KeyboardSettings::double_press_ms` of the first (Preferences
▸ Keyboard ▸ Double-press speed, 400 ms by default) and each press be
shorter than that. A yellow key cap with a letter goes on everything
that can be picked, as Vim's EasyMotion does. Typing a letter picks it; Backspace goes back a level;
Esc, Control (pressed once: it still goes on to be Control), a click or
any other key (which then does what it does) puts them away. Letters are by key position, as on a
US keyboard, home row first (`LETTERS`).

They must show at once, however busy the screen. What can be picked
comes from the kit's trees, asked for only the surfaces showing
(`{"hints":[surface ids]}`, which answers once those have drawn, with
only what can be picked: see the commands socket): the focused window's
controls, the other windows (by their tabs, which app_server knows
itself; picking one brings it forward and letters its content next)
and every layer surface on screen (Deskbar, the desktop's icons,
notifications; not one hidden behind windows, which draws once a second).
Each app is asked on a worker thread of its own, which also reads and
`build`s its answer (`ask_and_build`), 500 ms at most; the event loop
only prunes and letters. The asking starts ahead: Control let go on its
own is the keymap's `Action::HintsAhead` (`hints_ahead`), and the
second press shows the level asked for then (`Hints::ahead`, good for
`AHEAD`), or shows it the moment it is in; the gap between two presses
is longer than the apps take to answer, so the letters are there as
Control goes down. An app from before `hints` hangs up on it
and is asked `{"tree":true}`. How long they took is logged at debug
(`keyboard hints shown`). `build` turns a tree into `Node`s: what is clickable,
enabled and shows in the window, less a nameless area behind others (a
list's background) and the outside of a control around its own inside;
boxes (a widget's own nesting) are `Group`s. With a menu open, only its items,
the last opened (a submenu) first. `prune` drops what a click wouldn't
reach (a window in front: `clicks_reach` follows the surface under the
middle up to its window or layer). A level never has more than 26
letters (`expand`): all of it when it fits; else groups opened smallest
first while they fit, outlined in navigation blue, and what's left
after 25 letters in runs. Picking a leaf clicks its middle (the pointer
goes back after `SETTLE` if untouched) and then looks for a menu it
opened, whose items are lettered next; sliders, text fields and the
like are given the keyboard instead (`{"focus":ID}`), as a click would
change them. The badges are memory buffers in front of everything
(`Hints::elements`, from both backends), cached per letter, scale and
theme.

Agents read the letters showing with `{"type":"hints"}` (label, global
rectangle, name, role: AccessKit's, `Window` or `Group`) and type them
as keys.

## AI features (crates/ai, apps/smart-tile, Preferences ▸ AI)

Preferences ▸ AI (`olo-preferences ai`) holds `Settings::ai`
(`AiSettings`). Each feature is an `AiFeature { enabled, model, effort }`,
the model one of `AI_MODELS` and the effort one of `AI_EFFORTS`; the
panel's `feature()` draws its switch and choosers, so the next feature
is a struct field and a BBox: Smart Tiling, and Emoji (the emoji
picker's ⌘↩, low effort by default: it's asked while the person waits). `AI_MODELS` is OpenAI's current Codex
models (Astra 6, the default; Sol 6.1, Terra 6.1, Luna 6), reached
through the Token Pool's gateway (`http://127.0.0.1:{OLO_POOL_PORT or
17891}/codex`) and its Codex accounts. A model no longer listed (a
Claude model chosen before they were dropped) is read as the default
(`AiFeature::model_id`), keeping its effort. Claude is still spoken by
`olo_ai` for when it is offered again: through the pool's `/claude`
with `use_pool`, otherwise straight to Anthropic with
`olo_ai::saved_key()` (`$XDG_CONFIG_HOME/olo/anthropic-key` or
`ANTHROPIC_API_KEY`).

Every call introduces itself as `olo-ai/VERSION` (the pool lists its
requests by it) and is tried up to 4 times while the answer is 429,
503 or 529: after the `retry-after` it asks for, else 2, 4, 8 s plus up
to a quarter at random. Asked to wait over a minute (a quota resetting
later), it gives up with the server's message.

`olo_ai::ask` sends one question and returns the answer's JSON. For
Claude it is the Messages API (`anthropic-version: 2026-10-21`): the
system prompt, one user turn of content blocks (`png_block`,
`text_block`), `output_config: {effort, format: {type: "json_schema",
schema}}` (structured output), thinking left to the model (adaptive;
Opus 5.5 can't turn it off), and `fallbacks: "default"` with the
`server-side-fallback-2026-07-01` beta on every model but Haiku, so a
declined request is retried on another. `stop_reason` "refusal" and
"max_tokens" are errors. OpenAI's (`gpt-*`) go to the Responses API as Codex sends it
(`instructions`, `input_text`/`input_image`, `reasoning.effort` with max
as xhigh, `text.format` json_schema strict, streamed, not stored);
`openai_answer` reads the stream's text deltas or the completed
response. `OLO_AI_BASE_URL` sends every call to a fake server instead:
**tests never call a real model**.

Smart Tiling (`olo-smart-tile`, Smart Tile ⌃⇧⌘↩, default Astra 6 at
medium; a failure is logged to the journal as well as shown) is a process app_server starts, so nothing waits on
the model. It asks for the `Arrangement`, takes a PNG of that display
(before its notification is in it; at most 1568 px on the long side),
gathers what each kit app's windows show from their widget trees
(`commands::tree`, one thread per app, cut to 60 lines), and asks for a
rectangle per window in percent of the tiling area plus a one-sentence
summary. A progress notification ticks the seconds and has Cancel; the
answer is checked (known ids, once each, positive sizes; `arrange`
clamps to the area) and applied with `Arrange`; the last note says what
it did and offers Undo for a minute. One window just fills the area;
with the feature off it says so and offers Preferences. Test it with a
fake model:
`$T run NAME OLO_AI_BASE_URL=http://127.0.0.1:PORT olo-smart-tile`
(the fake answers `POST /v1/messages` with `{"stop_reason": "end_turn",
"content": [{"type": "text", "text": "{\"windows\": […], \"summary\": …}"}]}`).

## Power (crates/app_server/src/power.rs)

**Power modes** (`olo_system::power_profile`, Preferences ▸ Power,
Deskbar's battery menu): Performance, Balanced, Power Saver, through the
power profiles daemon's D-Bus API (`org.freedesktop.UPower.PowerProfiles`:
power-profiles-daemon, or Fedora's tuned-ppd; `net.hadess.PowerProfiles`
for older daemons), which makes each what the machine has: the
processors' energy/performance preference and governor (intel_pstate,
amd-pstate), the firmware's platform profile (a ThinkPad's power limits
and fans), tuned's settings. `power_profile::watch` follows the daemon's
signals (a ThinkPad's Fn+L/M/H change the platform profile, and the
daemon follows), so the panel and the menu show the mode in use and
why Performance is held back (on a lap, running hot). Setting it is
polkit's `switch-profile`, allowed to the person at the computer.
Performance has a button to Power, where people may look for it.

**The battery's charge limit** (Preferences ▸ Power ▸ Battery, where the
battery has one: `olo_system::battery::charge_limit`): UPower's charge
threshold (`EnableChargeThreshold`, polkit's to the person at the
computer): charging stops at its end (80 % on a ThinkPad) and starts
again below its start (75 %). Power is two columns (the power mode and
the battery; when idle and caffeinated), to fit 1280 × 800.

app_server owns idleness: any input (or a synthetic event) is activity; a
5 s tick turns the displays off (`settings.power.display_off_minutes`) and
then asks logind to suspend (`sleep_minutes`). Caffeinated mode
(`Request::SetCaffeinated`, the Deskbar cup), apps holding a
zwp_idle_inhibit inhibitor, and IPC connections that asked
`Request::KeepAwake` (for as long as they stay open: olo-portal holds one
for each app's wake lock, through the Inhibit portal or
`org.freedesktop.ScreenSaver`, `apps/portal/src/inhibit.rs`, released
when the app lets go or leaves the bus) count as activity. `Request::PowerStatus` and
`Event::PowerChanged` report it all, with the built-in display's brightness
(`Request::SetBrightness`, perceptual 0–1). Brightness, volume and
microphone keys are taken by app_server (the microphone key, a
ThinkPad's F4, KEY_MICMUTE: the microphone in use muted or unmuted,
`audio::toggle_input_mute`, a microphone on screen, slashed while
muted; when that's Olo's voice processing, the microphone it records
from too, so the sound stops at the hardware and the key's light, the
kernel's `audio-micmute` trigger on the card's capture switch, comes
on), as are a laptop's display, airplane and notification keys (a
ThinkPad's F7 KEY_SWITCHVIDEOMODE opens Preferences ▸ Display; F8
KEY_RFKILL turns every radio off or back on, `olo_system::rfkill`,
`/dev/rfkill` being the session's, an airplane on screen; F9
KEY_NOTIFICATION_CENTER opens the notifications' history, as Deskbar's
bell does; the phone keys, F10 and F11, and the star, F12, go to the
app in front and can be shortcuts); the brightness keys act on the
display under the pointer when its brightness can be set, else the
built-in one.

A **human presence sensor** (a ThinkPad P1 Gen 9's, in its camera
module: the HID sensor hub's IIO device "prox", `proximity0` someone
there, `attention` looking; `olo_system::presence`) locks the screen
when the person walks away (`PowerSettings::lock_when_away_seconds`)
and wakes the displays, asleep for idleness, when they come back
(`wake_on_return`), from Preferences ▸ Power ▸ Presence, shown only on
a computer with one. Both off by default; app_server watches the sensor
only while one is on (`presence_settings`): a thread waits on its buffer
(`/dev/iio:deviceN`, which carries a sample only as either changes),
`_raw` read once at the start (each read asks the sensor). Walking away
arms a one-shot timer, not a poll. `data/udev/73-olo-presence.rules`
(Olo Doctor's "rule:presence", and `tools/install-session.sh`) gives
the session the device and turns its buffer on, at 1 Hz.

Each display's brightness is `OutputInfo::brightness` (None: can't be set)
and `Request::SetOutputBrightness { name, level }`. Besides the built-in
backlight, monitors that take it over USB (Apple Studio Display and Pro
Display XDR, LG UltraFine: the USB Monitor Control class) are driven
through hidraw (`olo_system::hid_monitor`), matched to their connector
by the EDID maker; `data/udev/70-olo-displays.rules` (installed by
`tools/install-session.sh`) gives the session's user the device. DDC/CI
isn't reachable on Apple Silicon. app_server keeps each display's level in
`OutputConfig::brightness` and sets it when the display appears; with none
kept, every display starts at full brightness. Brightness belongs to
app_server: `SetSettings` keeps its own copy of these levels.

Hardware is touched only when app_server is the session
(`Backend::IS_SESSION`), from a worker thread: the backlight through
logind `SetBrightness` (no root needed), USB monitors, the inhibitor,
suspend, the sound server. Headless sessions simulate a backlight and a
USB brightness control on every display (all starting full) and only log
"would sleep", so tests can drive all of it safely.

## The screen saver (crates/screensavers, apps/screensaver, app_server saver.rs)

The modules are R5's own list, in its order (Blackness, BuyNow, Flip,
Icons, Lens, Lissart, Message, Spiral, Spots, StringTheme, SuperString;
from period screenshots of R5's ScreenSaver preferences). Each is a
`olo_screensavers::Module` that draws a whole frame into a rect on a
`Canvas` (rectangles, lines, polylines, discs, rings, triangles, text,
icons and the desktop picture, whole or as textured triangles; points
and colours are the crate's own `geom` types), so the same code runs
full screen and in the preview monitor of Preferences ▸ Screen Saver.
Both draw on GPUI (`gpui_canvas::GpuiCanvas`, feature `gpui`): it keeps
what a module draws and paints it at the end of the frame, a run of the
same kind of thing to a `paint_layer`, since hundreds of crossing lines
each inserted in the scene's bounds tree made Lissart and SuperString
two to three times egui's CPU. Size things with `Frame::k()`, step motion
with the fixed 60 Hz `Clock`, and add settings as `Opt`s (stored as strings
in `settings.screen_saver.options[module]`; Preferences builds their
controls). Modules with `uses_desktop` (Flip, Lens) get a picture of the
desktop that app_server takes as the saver starts.

app_server starts `olo-screensaver` after `screen_saver.run_minutes`
idle, from `Request::StartScreenSaver` (Test) or when the pointer enters
the "Fade now" corner, on a private connection (`spawn_private`, like the
lock screen); the "Don't fade" corner and caffeinated mode hold it off. Any
input disconnects it (after 1.5 s grace for a deliberate start). With
`screen_saver.lock` it locks underneath after `lock_minutes`, and keeps
drawing over the lock screen (`Lock::over`) until the first input. The
displays turning off ends it. app_server hides the pointer while it runs
(`saver::shown_cursor`, in every backend and in screenshots): the
saver's own `CursorIcon::None` only counts once the pointer is on its
surface, which takes a move, and a move ends it.

Test headless: `tsession ipc NAME '{"type":"start_screen_saver"}'`, or set
`screen_saver.module` with get/set_settings first.

## Fractional scaling (app_server render.rs, shell/deco.rs; the kit and GPUI)

A screen can be at 1, 1.25, 1.5, 1.75, 2, 2.5 or 3 (Preferences ▸
Displays; each is a whole number of 120ths, as wp-fractional-scale
sends it). At every one of them, nothing Olo draws is resampled, and
every line and edge is a whole device pixel. This is how modern
compositors and toolkits do it (KWin's pixel-grid snapping, Mutter,
Chrome, GTK 4.16 text), and how Olo does it:

- **Clients draw at the device size.** Olo's GPUI asks app_server for
  wp-fractional-scale-v1 and wp-viewporter on every surface, draws into
  a buffer of `round(size × scale)` pixels and sets the viewport's
  destination to the logical size (a window's scale is the fractional
  one, never an integer scale set over it: gpui/README.md). app_server
  places each window on a whole device pixel
  (`to_physical_precise_round`), so a buffer lands one to one. Programs
  without fractional scale get the next whole scale and are scaled down.
- **app_server's own pictures go pixel for pixel**
  (`render::exact`, `render::picture`). smithay sizes a memory element in
  whole logical points, so a picture drawn at its device size `w` came
  out `round(round(w / s) × s)` wide: 2561 pixels for a 2560 wallpaper
  at 1.5×, softly resampled. Wrapped in `render::Exact`, it fills exactly
  the rectangle it was drawn for. Every app_server picture goes through
  it: the wallpaper, tabs, the ⌘Tab switcher, the level indicator,
  Identify and the keyboard hints' badges. Lines (the hints' group
  outlines) are hairlines between corners rounded to the nearest pixel.
- **The decorator works in device pixels.** The frame is drawn outward
  from where the content lands: a border's width (`px(BORDER)`, the
  content's own offset) on every side, the same on all four.
  - *Its lines* are `deco::lines(scale)`. The outline is a hairline,
    `floor(scale)` (as olo-ui's `style::line_pixels`). The bevel's light
    and shadow are each `round(scale)`. The flat middle takes the rest.
  - *At 1.5×* that gives 1, 2, 3 and 2 pixels. Rounding each logical line
    from the window's corner gave 2, 1, 2, 1 and 2 on one side, and
    mirrored it differently on the other.
  - *Where the frame reaches:* it can end a pixel past `px` of its logical
    right and bottom. A tab ending there ends on it.
  - *The tab* is drawn the same way, its boxes inset equally from its own
    two ends, so every tab looks the same wherever it sits.
  - `frame_bands_at_every_scale` pins the bands at each scale.
- **The kit snaps what it works out.**
  - *Sizes* are on the 2-point grid (whole pixels at 1×, 1.5× and 2×).
    Positions it works out go through `style::snap` or `snap_centred`.
    Lines are `style::line_pixels`.
  - *The text caret*, menus' and tips' outlines are hairlines at each
    scale (`style::hairline`): a one-point stroke would be a pixel and a
    half at 1.5× and smear over two.
  - *An icon* is drawn at its texture's size from a whole pixel. A bitmap
    with no size of its own (ours come at 16, 24, 32 …, so 20 at 1.25×)
    is averaged down to exactly the device size once, when it loads (as
    Chrome scales its icons), instead of being stretched by the GPU
    every frame. Pictures are resampled exactly to their device size on
    a worker (`ImageView`).
  - *Viewer's pages* are drawn pixel for pixel once rendered for the
    current zoom and scale.
  - *An app that caches anything measured in pixels* keys it by the scale,
    as Terminal's cell metrics are (`Metrics::ppp`); a window can move to
    a display at another scale.
  - *At 1.25× and 1.75×* two points are 2.5 and 3.5 pixels. Edges are
    still crisp there (each is rounded), but spacing may differ by a
    pixel.
- **Text** is drawn by Olo's GPUI at the device size, each line's
  baseline on a whole pixel, as each display's text rendering says
  (Preferences ▸ Display: gpui/README.md): at most hinted vertically
  ("light", as FreeType, GTK 4.16 and Chrome do it) and positioned
  across in subpixel steps, so stems and baselines sit on pixels while
  shapes and spacing stay as designed. Tab titles (cosmic-text and swash)
  are hinted, on a whole-pixel baseline.
- **The pointer** is a vector drawn at each screen's exact scale (see the
  next section).

To check: the layout review flags fills, icons and outlines off the
pixel grid (`off-pixel`, `blurry-icon` with an icon a pixel off its
texture's size, `blurry-outline`). For the decorator, zoom into a
corner at each scale (`$T zoom NAME OUT X Y W H 8`), with the window at
an odd x so it sits between pixels.

## The pointer (crates/hvif, app_server cursor.rs)

Where it is drawn: a frame draws the pointer where the last reports say
it will be at the frame's vblank (`cursor::Track`, from the velocity of
the last two reports, at most a frame ahead and only while reports keep
coming and nothing warped the pointer; kept on the display). A mouse
reports at its own rate (125 Hz, 1000 Hz) against the display's (60,
120 Hz): drawn at the last position, one frame got two reports' worth
of motion and the next three, and the pointer moved unevenly though the
hand didn't (Ben, 2026-11-01: "jittery/off"); as `pace.rs` does for
scrolling, the frame clock gets the position for its time: where it was
4 ms before the vblank, interpolated between the reports either side of
then, or past the last report along the velocity of the last 6 ms of
reports by at most 4 ms (a frame's extrapolation at the last two
reports' velocity overshot every turn and was noisier than the real
position with a 1 kHz mouse: measured, `{"type":"pointer_trace",
"output":"DP-1"}` answers the last 120 frames that drew the pointer,
vblank, real and drawn position, the last report's age and interval).
Hit-testing, focus and what clients get use the real position.
`OLO_POINTER_AHEAD=0` draws the real position.

What the pointer is in follows what is under it, not only its moves
(Smithay looks again only on motion): a window that opens, closes,
comes forward or moves under a still pointer (Space in Tracker opening
Quick Look under it) takes the scroll or the click that comes next
(Ben, 2026-11-05: Quick Look didn't scroll until the pointer moved).
Before a scroll and a button press, `repoint` enters what is under the
pointer if that isn't what it is in, or is but moved (about 2 µs a
scroll event, 300 wheel notches measured, and no motion sent when
nothing changed); not while a grab has the pointer (a button held, a
drag, a menu). A window or layer that comes on screen, a window
brought forward, minimized, restored or closed asks for it once that
turn of the loop is done (`repoint_soon`), so the window under the
pointer has it, its cursor and its hover, without a move. GPUI takes
the place it is entered at as a move (`wl_pointer.enter` dispatches a
`MouseMove`): otherwise a GPUI window entered without a move kept the
last cursor and hover until the pointer moved.

The pointer's reports themselves are watched (`Track::health`,
`PointerHealth` in `{"type":"frames"}`, Activity Monitor ▸ Inputs'
Pointer card): reports a second, the device's interval (1 ms for a
1 kHz mouse), gaps over 5 ms within a run of motion in the last minute
and the longest. A gap is the mouse's, not a frame's: a receiver asleep
(USB autosuspend; `data/udev/71-olo-input.rules`, installed by
`tools/install-session.sh`, keeps every USB HID interface's device
awake) or a 2.4 GHz link dropping packets (Ben's Razer receiver on a
hub, 2026-11-01: frames of 67, 10, 50 px with reports 1 ms apart, and
"stuck for a second" after a pause). No frame can make up for a report
that never came.

The touchpad's reports are watched as they reach app_server
(`touch_health::Track`, `TouchHealth` in `{"type":"frames"}`, Activity
Monitor ▸ Displays' Touchpad card): reports a second and, over the last
minute, each one's latency from the kernel's time on it to app_server
handling it (libinput's work, Olo's touch filter and its palm model,
and any wait for the main thread, which draws too), typically, at the
99th percentile and the longest; whether the filter and the person's
palm model are in use (its training date and test numbers); and the
main thread's nice, realtime or not and `uclamp.min`. Noting a report
is 21 ns (`note_cost`); the minute's figures, 33 µs, are made when
asked. A 99th percentile over 4 ms (half a 120 Hz frame) is shown red:
touches waiting on frames being drawn.

The pointer is Haiku's system cursor set (`crates/hvif/cursors`, from
Haiku's CursorData.cpp, MIT; see the LICENSE there). `olo-hvif` reads
the Haiku Vector Icon Format and draws it as Haiku's IconRenderer does
(shapes in order, 2.2-gamma blending, level-of-detail, stroke and contour
transformers); `olo_hvif::cursor::render(shape, scale)` draws a cursor
as Haiku's CursorManager does: 22 logical pixels, the icon's top-left
32 × 32 region, a soft shadow, the hotspot scaled. app_server's
`cursor.rs` maps CSS cursor names (from its own decorations, or clients'
cursor-shape-v1 requests) to them, caches each shape per scale and
hands the backend a buffer whose pixels land one-to-one on the screen.
Clients that set a cursor surface still get theirs. To see every cursor:
`OLO_CURSOR_SHEET=/tmp/c.png cargo test -p olo-hvif sheet`.

When the displays sleep for idleness, `Backend::set_displays_on(false)`
clears every CRTC on DRM (DPMS off) and rendering stops, since any frame
would turn them back on; input, waking from sleep or a returning session
calls it with `true` and renders again.

## Every frame on time (app_server frames.rs; Activity Monitor ▸ Displays)

A display's frames are paced by app_server from each vblank: it waits,
then draws, so that clients told to draw at the vblank have time to hand
in their buffers and still make the next one. Smithay waits 60% of the
frame and draws in the last 40% (6.7 ms at 60 Hz, 3.3 ms at 120 Hz),
whatever the frame costs. A 5K frame with a video playing cost 5–9 ms of
CPU on asahi (2026-11-01), so a third of the Studio Display's vblanks
were missed, each a frame shown twice: the pointer jerked and scrolls
stuttered, and on the 120 Hz built-in display the 95th-percentile frame
(2.9 ms) was within a hair of the budget.

`frames::Pacing`, one per display, sets the split from what its frames
cost. Forward: the budget is at least the 95th percentile of the last
120 frames' CPU time, and a quarter more, plus 0.5 ms for the GPU. Back:
the vblank's hardware timestamp against the vblank the frame on its way
was drawn for says whether it made it; a frame or more late is a drop,
and a drop widens the budget by a quarter frame at once, since the GPU's
share isn't measured. Six hundred frames on time in a row narrow it by a
sixteenth, down to Smithay's split, because a narrower budget is a later
draw and less latency for everyone. The pacing costs nothing while a
display rests, and a look of a display's own (woken, or its idle look)
isn't judged, since it wasn't drawn for a vblank.

The pointer goes on a plane of the display's own where there is one.
Apple's display controller (apple-drm) has no cursor plane, so every move
of the pointer was a frame of the whole screen, and when one ran long the
pointer waited with it. It has two overlay planes a CRTC that take
ARGB8888 (how macOS shows its pointer): the topmost becomes the cursor
plane, and Smithay draws the pointer into a 64×64 buffer of its own and
moves that, the frame underneath untouched. The log says
`no cursor plane; an overlay plane holds the pointer` when a display is
set up so, and `the pointer is on a plane of the display's own` the first
time it goes there; a pointer the plane refuses (bigger than 64 px, a
client's cursor it can't hold) is drawn into the frame as before.
`OLO_CURSOR_PLANE=0` keeps the overlay planes as they are. Olo's Smithay
keeps the plane on the display (clipped at an edge; apple-drm refused a
plane hanging off the display with EINVAL on the page flip, and the
first try took the desktop down with it), and a display that refuses a
frame's commit draws everything from then on (`planes_refused`), the
pointer too: a refusal costs a frame, never a restart.

Olo's Smithay also checks every rect in `Rectangle::
subtract_rects_many_in_place` (which the damage tracker uses to find
what an opaque region stopped covering): swapping the last rect in for
one taken out could bring in a piece made in the same pass and leave an
earlier rect unchecked, so a frame's opaque regions weren't all taken
from themselves and the same strip was damaged every frame: with any
window floating over Focus's, app_server drew 60 frames a second while
nothing changed (2026-11-02; `subtract_rects_many_checks_every_rect`).

Where a frame's time goes: measured on Ben's session (2026-11-01,
read-only stack samples of app_server's main thread, and Smithay's
phases through `PHASE_HOOK`), the 5 ms a 5K frame costs is all
`render_output`, and inside it the GPU submit (Mesa's asahi driver
flushing the batch for the fence Smithay makes finishing a frame) and
syncobj waits: app_server sits in the kernel handing the composite to
the GPU, about 3 ms a frame, 48 times a second for a video call, which
is its 20% of a core. Plane assignment and the atomic test cost 0.0 ms.
So the way out is fewer composites: the pointer on its plane, and a
window whose buffer changes every frame (the call) straight to an
overlay plane, which Olo's Smithay now tries for every window's own
surfaces (`Kind::ScanoutCandidate`); the display's "scan-out" record
(`DisplayFrames::scanout`, Activity Monitor ▸ Displays) says how many
elements went to a plane and why the rest were drawn ("overlaps": an
element above it on the primary plane; "format": its buffer isn't one
the plane takes; "the driver refused": the atomic test failed; "not
tried": not a candidate).

`{"type":"frames"}` (`tsession ipc NAME '{"type":"frames"}'`) answers
each display's record (`DisplayFrames`): frames a second, dropped in the
last minute and in all, the draw's percentiles, the budget and delay,
whether the pointer is on its own plane, resting. Activity Monitor's
Displays section shows it, a card a display with graphs of its frames
a second, drops and the 95th percentile of drawing over the span
chosen, asked every second while the section (or Inputs) shows and
never otherwise; the answers are kept for the graphs
(`Activity::frame_history`, half an hour). Its Inputs section has the
pointer's and the touchpad's cards (each with a graph: reports a
second, the touchpad's latency typically and at the 99th) and What
You Did: clicks, scrolling steps, gestures begun and keys pressed in
the last minute (`InputActivity`, app_server's `input_counts`: a time
noted per event as it's handled, real or synthetic, how many and never
which key), with a graph of each. Its check box, "Capture debug
information whenever a frame drops" (`{"type":"capture_drops","on":true}`),
has app_server log every drop as a `frames` record: how late, how long
the draw took and its parts (gathering, `drm render_frame`, …), the
budget, and what else held the loop meanwhile (the slowest request,
input event or commit since the last vblank); Logs filtered for
"frames" shows them. It stays on until app_server restarts.
`OLO_ACTIVITY_DEMO_FRAMES=1` gives Activity Monitor made-up displays to
look at in a headless session, which has none.

## Caffeinated mode and the level indicator (power.rs, osd.rs)

Caffeinated mode holds a logind block inhibitor ("sleep:idle", plus
"handle-lid-switch" when asked to survive the lid) and keeps idleness at
bay; `Request::SetCaffeinated { on, minutes, lid }` sets it, the power
tick ends a timed stretch, and `PowerStatus` carries `caffeinated_until`
and `caffeinated_lid`. Deskbar toggles it from the cup (right-click, or
Olo ▸ Stay Awake, for durations and the lid); Ctrl+⌘A toggles it from
the keyboard. The keyboard backlight (`olo_system::backlight::keyboard`, an LED
class device set through logind) follows its keys and Preferences ▸
Keyboard, and goes off with the displays. The brightness, keyboard and
volume keys and Ctrl+⌘A show `osd.rs`'s
panel low on the screen under the pointer for 1.6 s (in a real session
the worker reports the volume it set; headless simulates a volume).
`OLO_OSD_SHEET=/tmp/o.png cargo test -p app_server osd_sheet` draws
every state.

## Approving without asking (app_server approve.rs, Deskbar approve.rs)

For a while (5 minutes to a day), requests that would ask for the
account password are approved without asking: sudo in Terminal's window
and Passwords' unlock and `op` access. Deskbar's key in the tray turns
it on (a click: for 15 minutes, `approve::USUAL`; right-click or Olo ▸
Approve Without Asking: the lengths) in a small window that takes the
password; the key is gold while it is on, its tip saying until when,
and a click (or Ask Again) turns it off.
`Request::SetAutoApprove { minutes, password }` (`minutes: None` turns it
off; while on, a new length needs no password) checks the password
through PAM on a thread and replies once it has ("That password isn’t
right."). app_server keeps the password (`Zeroizing`) only while it is
on: the power tick ends it at its time, the lock ends it (sleeping locks
first), and app_server's restart ends it; `PowerStatus` carries
`auto_approve_until`. Terminal's sudo window, before showing, asks
`Request::AutoApproveSecret` (answered only to a peer that is Olo's
olo-terminal, `Conn::peer_is_ours`) and gives sudo that password;
Passwords asks `PowerStatus` (on a thread) and unlocks, or lets a
terminal's `op` in, without its window. Each says so in a quiet
notification ("Approved without asking", "Passwords unlocked without
asking"). Never test with the real PAM: debug builds take
`OLO_PAM_CONFDIR`/`OLO_PAM_SERVICE` (a stand-in `pam_exec` check),
Terminal's sudo `OLO_SUDO_REAL`, and `OLO_NO_FINGERPRINT=1` leaves
fprintd alone.

## Security keys (olo-fido; Preferences ▸ Security; app_server security_key.rs)

A security key set up (a YubiKey: any FIDO2 key with `hmac-secret`) is
touched instead of typing the account password: Terminal's sudo window,
Passwords' unlock, the polkit agent (for the person's own identity) and
the lock screen. Natively, as the person, no root: `olo_fido` speaks
CTAPHID to the key's FIDO interface (its hidraw node, found by usage page
0xF1D0 under /sys, open to the person at their seat by systemd's
uaccess) and CTAP2 itself (GetInfo, ClientPIN's key agreement in PIN
protocol 2 or 1, MakeCredential, GetAssertion with `hmac-secret`).

* **Setting up** (Preferences ▸ Security ▸ Security Keys, `security_keys.rs`):
  the box says the steps first (the password, then a touch when the
  key's light blinks, of its gold contact, on a Nano the gold edge at its
  end; a touch before it blinks types a code, which app_server swallows,
  so nothing happens), then what to do as each touch is waited for, with
  the touches had. A key stops waiting for a touch after about 30 seconds
  (a YubiKey answers "operation denied"): each touch is asked for again,
  three times in all (`olo_fido::TOUCH_TRIES`, `Asked`), saying so, and
  one never touched is "The key wasn't touched in time." (Ben's first
  setup ran out while he touched the wrong part of his key: the box
  said only "touch the key twice".)
  The password is checked through PAM, then two touches: a credential
  made for Olo (relying party `olo.local`, not discoverable,
  `hmac-secret` on), then its secret for a random salt, which seals the
  password (XChaCha20-Poly1305 under HKDF of it, the user and credential
  as associated data). Kept in `security-keys.json` in the config
  directory (0600): useless without the key. A second key set up the same
  way is a spare; Remove forgets one. The box follows keys plugged in and
  out from udev's own announcements (`olo_fido::hid::watch`, netlink
  group 2: sent once the node's permissions are set; no polling), asking
  each what it is and whether it's one set up without a touch
  (GetAssertion with `up` off). A key with a PIN set that wants it for a
  credential isn't supported yet (it says so).
* **Unlocking** (`olo_fido::touch`): each window, as it asks for the
  password, starts a wait on a thread of its own: if a key set up is
  plugged in it says `olo_fido::TOUCH_OR_TYPE` ("Touch your security key,
  or type your password.") beside the field, and the password the touch
  unseals goes where a typed one would (sudo's prompt, the polkit
  helper's, PAM through `olo_system::auth::check_password` for Passwords
  and the lock screen, so a password changed since setting up fails as a
  wrong one would). The wait is dropped with the window (CTAPHID_CANCEL),
  and Passwords and the lock screen wait only while their window has the
  keyboard. The key gives up on a touch after about 30 seconds; the lock
  screen asks again three times, then not till someone is there (a key,
  the pointer), so a locked computer's key doesn't blink all night.
* **Stray touches** (app_server `security_key.rs`): a touch with nothing
  waiting types the key's one-time code through its own USB keyboard.
  app_server knows that keyboard by the device as it's added
  (`olo_protocol::keyboards::security_key`, by its USB id from then on)
  and passes its keys to nobody; three touches in a row (each within
  three seconds of the last) type the third's code into the focused
  window as the key would have (`SecurityKeySettings::three_touches_type`,
  on by default: Preferences ▸ Security ▸ Three touches type a code).
  While a window waits for the key a touch answers that instead (FIDO),
  typing nothing at all.
* **Testing**: never with the person's key or password. Debug builds take
  `OLO_FIDO_SOFT=FILE`, a software key (`olo_fido::soft`: the same
  CTAPHID and CTAP2, its master secret in FILE so a credential made in
  one process opens in another) that touches itself
  `OLO_FIDO_SOFT_TOUCH_MS` (400) after it starts waiting, or never
  ("never"); with the PAM stand-in (`OLO_PAM_CONFDIR`/`OLO_PAM_SERVICE`),
  `OLO_SUDO_REAL` and `OLO_POLKIT_HELPER` the whole thing runs headless.
  `cargo run -p olo-fido --example info` asks every key plugged in what it
  is, without a touch; `--example touch` runs setting up's two touches on
  the first key plugged in (nothing kept, no password), every CTAPHID
  message printed with when, to see what a key does. `cargo test -p olo-fido` (both PIN protocols
  against the soft key, cancelling, sealing).
* **Olo Doctor** (`security_keys`): a key plugged in whose FIDO node isn't
  the person's to open is a finding, its fix
  `/etc/udev/rules.d/74-olo-security-keys.rules` (systemd's `fido_id`
  and `uaccess`, for systems whose own rules don't), applied to the keys
  there with a `change`; one that is, a fine line.

## Face unlock (olo-face, olo-faced, pam_olo_face; Preferences ▸ Security)

A look at the infrared camera stands for the account password: sudo
(Terminal's window, any terminal), polkit's requests, Passwords' unlock
and the lock screen. Natively and in process: no howdy, no OpenCV
binaries, no linux-enable-ir-emitter.

* **Only infrared counts.** The camera is the one whose node gives only
  grey (`camera::infrared`: the ThinkPad's "Integrated IR Camera",
  640×360 GREY at 15 frames a second), read over V4L2 by raw ioctls
  (`camera::Camera`: S_FMT, memory-mapped buffers, poll and DQBUF). A
  face on a screen isn't there at all in infrared, displays giving off
  none. A face must be near (80 pixels wide), lit by the flood, and its
  relief (the middle of the face against its sides, which turn away from
  the light) like the one enrolled, since a print is lit evenly; two
  frames must match (`look::verify`). With no face at all for 2 s the
  look ends.
* **The emitter** (`emitter.rs`): a camera that waits to be told to
  light it (Chicony's, as Windows Hello's driver does) is told through a
  vendor control in one of its UVC extension units (`UVCIOC_CTRL_QUERY`).
  Olo finds it once, at the first enrolment, as Windows' setup would:
  each control of the infrared interface's units (read from the USB
  descriptors; on the ThinkPad units 10, 13 and 14) that can be both
  read and set, each value its range suggests, set while the frames are
  watched for the flood. The one that lights it is kept per camera (by
  its USB id, in `emitters.json`) and set each time the camera opens;
  everything else tried is put back as it was.
* **The models** (`model.rs`, `data/face`, Git LFS): YuNet (OpenCV
  Zoo's detector, MIT) finds each face and its five landmarks in a
  640×640 letterbox (its three strides decoded and overlaps dropped, as
  OpenCV does); SFace (OpenCV Zoo's recogniser, Apache 2.0) turns the
  face, aligned to its template by those landmarks (a least-squares
  similarity), into 128 numbers, compared by cosine similarity (a match
  at 0.42, above SFace's own 0.363, for the margin infrared and a lock
  want). The grey goes into all three channels. On the public-domain
  NASA portraits (`OLO_FACE_SAMPLES=DIR cargo test -p olo-face
  recognises -- --ignored`): two photographs of Armstrong five years
  apart 0.59, other men 0.09 to 0.26; 25 ms to find a face, 28 to
  recognise it.
* **olo-faced** (root, the system bus's `org.olo.Face1`, started by
  systemd when first asked) alone opens the camera, runs the models and
  keeps each person's enrolled looks (`/var/lib/olo-face/UID.json`,
  root's only), so nothing running as the person can read the faces or
  skip the look. Who asks is known from the bus
  (`GetConnectionUnixUser`): a person may ask about themselves, root
  about anyone. `Status`, `Verify(user, ms)`, `Enroll(user, password)`
  (with `Progress` signals) and `Forget(user, password)`; enrolling and
  forgetting check the password through PAM first, so something running
  as the person can't put its own face in. No password is kept anywhere:
  a look doesn't unseal one, it answers PAM. The camera is opened for a
  look and closed after it.
* **The PAM module** (`pam_olo_face.so`, `auth sufficient` first in
  `/etc/pam.d/sudo` and `/etc/pam.d/polkit-1`): asks the service (on the
  system bus, as fprintd is reached: polkit's helper runs as
  `policykit_auth_t` under SELinux), says "Looking for your face…"
  through the conversation, and succeeds on a match. Anything else (no
  service, nobody enrolled, no camera, not them, a remote login) goes on
  to the password: it never fails a login, it only spares the typing.
  It links libpam by file name (`+verbatim`), so it builds without
  pam-devel.
* **Passwords and the lock screen** ask the service themselves
  (`olo_system::face::verify`): Passwords once each time its locked
  window takes the keyboard, the lock screen when someone is there (a
  key, the pointer) at most every 5 s, so the camera's light isn't left
  on.
* **Installing** (Preferences ▸ Security ▸ Face Unlock ▸ Install…):
  `pkexec olo-faced install --models DIR --pam FILE` (Olo's polkit agent
  asks) copies the service to /usr/local/libexec, the models to
  /usr/local/share/olo/face and the module to /usr/lib64/security,
  writes its systemd unit, D-Bus activation and bus policy, puts the
  module's line in sudo's and polkit's stacks (polkit's from
  /usr/lib/pam.d copied to /etc/pam.d, which wins), and has systemd and
  the bus read theirs again; `uninstall` (Remove) takes it all out. The
  service and module come from beside Preferences or the tree it was
  built in (`share/olo/build-dir`). Set Up… takes the password and five
  looks (each a little turned), Set Up Again replaces them, Forget… drops
  them.
* **Testing**, without root, a camera or anyone's face: a private bus
  (busd), a debug `olo-faced serve --bus A --root DIR --models DIR
  --camera-dir FRAMES` (pictures for frames: only on its command line,
  never from its environment; a debug build takes the PAM stand-in,
  `OLO_PAM_CONFDIR`), and `cargo run -p olo-face --example client
  status|enroll|verify|forget|pam` with `OLO_FACE_BUS`; `pam` runs PAM's
  check with the module first in a stand-in stack. Tried so: a wrong
  password refused; five looks enrolled from crops of one portrait in
  1 s; another photo of him matched in 0.7 s, another man "not you",
  empty frames "no face"; through PAM, his face let in a wrong password,
  the other man's didn't, and the right password still did after the
  look.

## Each display on its own GPU (app_server udev.rs; Preferences ▸ Display ▸ Graphics)

Each display is composed by the GPU it is wired to (`render_surface`:
the surface's own render node), so no frame crosses between GPUs: on a
hybrid laptop the built-in panel and the USB-C ports are Intel's and
the HDMI port NVIDIA's, which wakes only while a display is plugged in
there. Composing the panel on NVIDIA's had every frame copied to
Intel's through the CPU (NVIDIA can't render into the linear buffers
Intel can import: `--probe-gpus` says which formats each GPU can render
and import linear), 11 ms a frame: 30 Hz (Ben: "we should just use
whatever the native one is they need to use"). Only a display device
that can't render (a USB adapter) has its frames composed on the
primary GPU (the one the computer started on) and copied, at 8-bit
(`composed_elsewhere`). Windows' buffers from another GPU are imported as
they come. Display's Graphics box (two or more GPUs, `Request::Gpus`:
`olo_system::gpus`) says what each drives.

### Variable refresh rate (VRR)

A display that can vary its refresh rate (the kernel's `vrr_capable`
and the CRTC's `VRR_ENABLED`: a ThinkPad P1 Gen 9's panel, 40–120 Hz)
says so in `OutputInfo::vrr` (None: it can't; `Backend::vrr`, from
Smithay's `vrr_supported`), and Preferences ▸ Display shows "Variable
refresh rate" under its refresh rate. It's the person's: off unless
turned on (`OutputConfig::vrr`, kept per display and set again as the
display appears), through `Request::ConfigureOutput`; app_server asks
Smithay (`use_vrr`), which changes it without a modeset where the
driver can and with the next frame's otherwise. Headless screens
simulate one, so the switch can be tried and reviewed.

### The discrete GPU stays asleep

NVIDIA's GPU on a hybrid laptop sleeps (D3cold) while nothing uses it,
and whatever loads NVIDIA's drivers wakes it and waits for it, ~2.3 s:
every GPUI app's start took 2.5 s instead of 77 ms (DeskCalc, its
first window), Chrome's GPU process the same, and each app_server start
(a reload's too) opened it. Nothing routine may wake it:

- app_server leaves NVIDIA's card unopened while nothing is plugged
  into it (`left_asleep`: its connectors' status, as sysfs keeps it,
  read without asking the GPU), and opens it when a udev change says a
  display is.
- GPUI's apps make their Vulkan context without NVIDIA's driver
  (`blade_context::asleep_aside`: `VK_LOADER_DRIVERS_DISABLE=*nvidia*`
  for the moment the loader looks, put back at once so nothing the app
  starts inherits it); they draw on the GPU the displays use, and a
  window on NVIDIA's HDMI display is imported there.
- Programs Olo starts get `olo_kit_base::gpu::asleep_env()` (Vulkan's
  loader and glvnd's EGL without NVIDIA's): `olo-chrome` does.
- Activity asks NVIDIA's library only while the GPU is awake, Doctor
  and `olo_system::gpus` read sysfs attributes only (never a PCI
  device's `config`, which wakes it), and no local model (voice,
  palm, olo-complete) has a GPU to run on: the CPU's SIMD.

The person decides otherwise by setting Vulkan's or glvnd's variables
themselves, or `__NV_PRIME_RENDER_OFFLOAD=1`: those are left alone. To
check: NVIDIA's `power/runtime_status` stays `suspended` and its
`power/runtime_active_time` doesn't move through a launch.

### Power: the processors' and each GPU's, by mode and on battery (Preferences ▸ Power, `olo-gpu-tune`)

Every power setting is in Preferences ▸ Power (Ben, 2026-11-06:
"centralize the gpu power settings and cpu power settings to all be
under the prefs -> power section, and have all these scenarios
configurable with reasonable defaults"), in three tabs:

- Power Modes: the power mode (Power Saver, Balanced, Performance), and
  a grid of what each mode, and running on battery, puts the processors
  and each GPU on (`gpu_tuning::Plan`; each cell a `MenuField` named by
  its row and column for agents, "Processors in Power Saver"). The
  processors' presets (`cpu_tuning::CpuPreset`) are what the power
  profiles daemon sets for its modes, so each can be chosen anywhere:
  Saving Power (the powersave governor, the "power" energy preference,
  turbo off, the firmware's low-power profile), Balanced
  (balance_performance, turbo, balanced) and Fastest (the performance
  governor, turbo, performance). A GPU's are Cooler and Quieter,
  Balanced, Fastest (`gpu_tuning::preset`) or Custom (its own knobs, on
  Graphics). The On Battery column is "As the Mode" or a preset that
  holds whatever the mode. The defaults: each mode's own, and on battery
  the mode's, but NVIDIA's GPU on Cooler and Quieter (video's Automatic
  wakes it on battery too). Only choices that differ from the defaults
  are kept (`Plan::chosen`), so a GPU added later takes its defaults.
  Below: what's in force now ("Now Performance, on battery: the
  processors Fastest, Intel Graphics Fastest, NVIDIA RTX PRO 2000 Cooler
  and Quieter").
- Graphics: a box for each GPU (`olo_system::gpus`: what it drives,
  whether it's awake, the preset it's on now and what that has it on in
  numbers (`gpus::in_force`: xe's clocks as its driver says now,
  NVIDIA's as they were set, which its driver doesn't say back; Ben,
  2026-11-07: "changing the power mode in prefs -> power doesn't change
  the clock setting of the nvidia gpu?", its box showing only Custom's
  slider), and its settings. On a preset they show what it runs at
  now (`gpus::shown`: xe's clocks and profile as its driver says,
  NVIDIA's as set; Ben, 2026-11-07: "it says nvidia is on cooler and
  quieter but the slider hasn't changed?", they had been Custom's alone
  under a heading of their own), and moving one makes it Custom where
  it is now (`gpus::situation`: on battery when the plan has a choice
  for it there, else the power mode), starting from those, so nothing
  else jumps: "Custom in Performance once applied (on Cooler and
  Quieter until then)", Revert back to the preset. On Custom they're
  Custom's, a knob not set showing what Custom would set, the driver's
  own (moving xe's lowest clock with no highest set had pinned the
  highest to it):
  Intel's xe the render GT's lowest and highest clock, `freq0/min_freq`
  and `max_freq` within `rpn_freq`..`rp0_freq`, and every GT's power
  profile; NVIDIA's, through NVML in process, its highest GPU clock,
  locked clocks from its rest clock, its power limit where it can be
  given one, a laptop's can't, and persistence, its driver kept ready so
  it never sleeps), then Screen Recordings and Video Playback (with
  "Automatic wakes NVIDIA's GPU on battery too", on by default).
- Battery & Idle: the battery and its charge limit, when idle,
  caffeinated and presence; the panel's Defaults and Revert are this
  tab's (the others have Apply and Revert of their own, for the
  computer).

The old GPUs panel's names (`gpus`, `gpu`, `graphics`, `nvidia`) open
Power on Graphics (`--tab` picks a tab: `power::show_tab`).

What a GPU allows is read without root (`gpu_tuning::caps`); a sleeping
NVIDIA GPU isn't asked (it would wake): what it said last time is shown
(`~/.cache/olo/gpus/`), or "Ask It". Asked and not answering, its box
says what the driver said (`caps_or_why`: "its driver didn't answer (an
internal driver error occurred)"), and that a restart starts its driver
again when the kernel has its power management in an error state
(`power_failed`: `runtime_status` "error", as NVIDIA's was after memory
ran out on 2026-11-07), with Ask Again; "Asking it…" while it asks.
NVIDIA's library is started again at each ask until it starts once (a
failed start had been kept for the life of the process: Preferences
never asked again). `olo-gpu-tune show` says the same, as
`power_failed` and `unanswered`. Apply runs `olo-gpu-tune set`
through pkexec (only in the person's own session: `OLO_REAL_SESSION`),
which keeps the tuning in `/etc/olo/gpus.json` (the computer's: it's the
hardware's), notes each GPU's limits as its driver had them before it
was first tuned (xe keeps whatever is written, and its own lowest clock
isn't one it says: Balanced writes those back), applies it, and installs
a root-owned copy of itself (`/usr/local/libexec/olo-gpu-tune`) and
`/etc/udev/rules.d/73-olo-gpu-tune.rules`, which applies it again as
each GPU appears at start (a rule running a program the person's account
could change would run anything as root). Agents read it all with
`olo-gpu-tune show` (JSON; `--wake` asks a sleeping GPU too).

The plan is followed as the power mode changes, wherever it's chosen
(Preferences ▸ Power, Deskbar's battery menu, a ThinkPad's Fn+L/M/H,
the daemon's own tools), and as the power supply does (UPower's
signals, `battery::watch`): the daemon sets the processors and the
firmware for its mode first, then each GPU and the processors are put
on what the plan says (`gpu_tuning::scenario`, `cpu_tuning::apply`;
`Plan::cpu_now`, `Plan::gpu_now`).

The daemon is the person's to drive without a password, but GPU clocks,
power limits and the processors' knobs are root's, so a root service
does it:
`set` also installs and enables `olo-gpu-follow.service` (systemd's own
API, `olo_system::systemd`: reloaded and started again when it or the
installed program is new), which runs `olo-gpu-tune follow`: it waits
for the daemon's name on the system bus (`power_profile::
wait_for_daemon`, the bus's `NameOwnerChanged`), then for its mode
changes (`power_profile::watch`, its `PropertiesChanged`) and UPower's
(mains or battery), and on each keeps the new tuning and applies it as
`set` does (`follow_now`: a sleeping NVIDIA GPU asked, waking it a
moment, as applying its preset does anyway; the processors' preset
after the daemon's own). It's woken by nothing else: idle, it costs
nothing. It holds NVIDIA's GPU open (NVML, opened at its first change
and never closed: `gpu_tuning::nvml`), and that is what keeps the GPU's
clock lock: with persistence off, NVIDIA's driver shuts a GPU down
whole when the last program holding it closes it, its lock and power
limit with it (open-gpu-kernel-modules' `nv_close_device` →
`rm_shutdown_adapter`), so a lock set by a program that then exits
lasts only while another has the GPU open (Ben, 2026-11-07: "changing
the power mode in prefs -> power doesn't change the clock setting of
the nvidia gpu?"). Held, the GPU still sleeps: on the ThinkPad (driver
615, fine-grained runtime D3) it suspended 25 s after its last use with
NVML open, video memory off; closing NVML is what woke it. `set` keeps the whole tuning (the plan, each GPU's Custom
knobs) and applies what's in force now. One password, at the first
Apply; Power Modes offers Apply while the service isn't there, or is
from an older build (the installed program compared with the
`olo-gpu-tune` beside Preferences, on its reading thread), and
otherwise says what's in force now. The panel follows
`/etc/olo/gpus.json` (inotify) so what's shown changes with the mode
and the supply. On the ThinkPad
(2026-11-04): Power Saver put the Intel GPU on 650–1600 MHz with its
power-saving profile and the NVIDIA GPU on 180–2160 MHz, and all 16
processors on the "power" preference, turbo off, the platform profile
low-power.

## The touch screen (app_server input_handler.rs)

A touch screen (a laptop's, as the ThinkPad P1 Gen 9's Wacom panel) is
mapped to the built-in display (eDP, else the first). Where a finger
comes down on a client that has `wl_touch` (Chrome, GTK), it gets
touch; anywhere else (every GPUI app, so every Olo app, and app_server's
own frames, tabs and menus) the finger is the pointer (`takes_touch`,
`touch_pointer`, `TouchPointer`): a tap clicks where it lands, a finger
that moves past 6 points drags (its button pressed where it came down),
so windows move, menus open and buttons click as with a mouse; a second
finger before the first has pressed makes the two scroll what's under
them (their middle's movement as a finger scroll, inverted: the content
follows the fingers; a lift stops it), or, once they've moved 8 points
and their distance apart changed more than their middle moved, pinch
(the pointer's pinch gesture, `zwp_pointer_gestures_v1`, as a
touchpad's). The pointer hides while a finger
is it, and shows again as a mouse or touchpad moves
(`touch_hides_pointer`).
The pen is a tablet tool (`zwp_tablet_v2`). Agents and tests touch with
`SyntheticEvent::Touch { slot, phase, x, y }` (`tsession touch NAME X Y`,
`tsession touchdrag NAME X1 Y1 X2 Y2`), which goes the same way.
A display's touch screen can be turned off (Ben, 2026-11-06):
Preferences ▸ Display's "Touch screen", shown for the display a touch
screen is on (`OutputInfo::touch`: Some while app_server counts a touch
device, `touch_screens`, on `touch_output`), on by default and kept off
by its key as its other settings are (`OutputConfig::touch`,
`DisplaySettings::touch_for`; on isn't written). Off, a finger coming
down or moving on it is ignored (`touch_on`); one already down still
lifts, the pen goes on working, and agents' synthetic touches are taken
whatever it says (they drive it, not the hardware). Agents set it with
`ConfigureOutput { touch }`.

## The touchpad (app_server udev.rs, momentum.rs, touch_filter.rs)

[TOUCHPAD.md](TOUCHPAD.md) has the measurements and the reasoning.
Preferences ▸ Touchpad & Mouse has four tabs (Touchpad, Touch, Gestures, Mouse)
and a tuning profile (protocol `touchpad.rs`: `PROFILES`,
`apply_profile`; changing any tuning by hand makes it "custom"); the
pad and the profile it suits come from `Request::InputDevices`
(`DeviceKind::detect`), and again from `Event::InputDevicesChanged`
whenever a pointing device comes or goes (`EventKind::InputDevices`:
the panel waits on it, nothing is polled). Headless, a test names the
devices in a JSON file (`OLO_HEADLESS_INPUT_DEVICES`, as
`Reply::InputDevices` lists them).

**Mice** (protocol `mice.rs`, the Mouse tab): each mouse is named by
maker and model (`mice::identify`: its USB or Bluetooth ids first, then
the name its driver gives it, with the maker's vendor id; `MODELS` holds
only ids seen on real devices and names as drivers give them; `maker`
names the rest by vendor), with its sensor and resolution, the
hardware database's `MOUSE_DPI` (read from udev's database by
`device_facts`, with the bus and whether its USB device may sleep), and
its quirks (`Quirk`: buttons on its motion node, which key remappers
grab; a receiver that must stay awake; a report rate above the
displays'; free-spinning and thumb wheels; a touch surface). A mouse
follows the other mice (`Settings::mouse_speed`,
`touchpad.mouse_accel_profile`), its model's profile
(`Settings::mice[key] = MouseTuning { profile: "recommended" }`:
`Class::Gaming` no acceleration, `Class::Everyday` adaptive), or its
own speed and acceleration (`profile: "custom"`); app_server's
`configure_pointer` takes `mice::effective`. What gets in a mouse's way
(a remapper's virtual pointer in the list, a receiver allowed to sleep)
is shown with a button that opens Olo Doctor, which fixes it.
A **pointing stick** (`InputDeviceInfo::pointing_stick`: named so,
scrolled on a held button by libinput's own default, or the PS/2 mouse
on a ThinkPad's keyboard controller, `isa0060/serio1`, which is its
TrackPoint when the kernel didn't know it as one: the P1 Gen 9's comes
as "PS/2 Logitech Mouse") is shown as the TrackPoint, at the top of the
second column, and scrolls while its middle button is held
(`TouchpadSettings::pointing_stick_scroll`, on: libinput's
`ScrollMethod::OnButtonDown` on BTN_MIDDLE, which it gives only the
sticks it knows).

On MacBook pads (Apple MTP) app_server loads
**Olo's touch filter** into its libinput: `touchpad.lua`, a libinput
1.30 Lua plugin written with the settings filled in to
`$XDG_RUNTIME_DIR/olo/libinput-plugins` (`touch_filter::install`). It
counts a touch only while it presses, reports palms and hovering
fingers with no size, and makes the click from the fingers' pressure
alone. libinput drops pressure on pressure pads, so with the filter on
`install` also writes a quirk, `$XDG_RUNTIME_DIR/libinput/olo-touchpad.quirks`,
that takes `INPUT_PROP_PRESSUREPAD` away, and the plugin clears the
pressure axis's resolution (docs/TOUCHPAD.md). Plugins and quirks are
read when libinput starts, so `start_libinput` makes a new libinput
context when the filter's settings change.
`cargo test -p app_server touch_filter` runs the plugin in mlua against
scripted frames; `examples/touch_filter_probe.rs` runs it on a real
device (root, read-only).
The filter also runs a **palm model** (`use_palm_model`, on by default;
`touch_filter::model_in_use`): the person's, trained on their hands
(`TouchFilter::palm_model`), or else Olo's own, made from real hands
(`olo_palm::default_model`, written by `make_default_model` from the
trainer's kept runs; crates/palm, `olo-palm`; docs/TOUCHPAD.md "Palms
told apart by a model"). Its live samples come
from app_server's `contacts.rs`, which reads the pad's slots with
`EVIOCGMTSLOTS` on a copy of libinput's descriptor while anyone
subscribes to `EventKind::TouchContacts` (only by name), for every
touchpad libinput opens (`INPUT_PROP_POINTER` without
`INPUT_PROP_DIRECT`, with contacts' places), not only those the filter
filters. A pad that reports no contact size (a ThinkPad P1 Gen 9's
Goodix GXTP5420: position, pressure and the firmware's tool type) gives
none (`PadAxes::size_from_pressure`; its pressure is the haptic click's
force, not an area), each contact says what the firmware takes it for
(`TouchContact::tool`: 2, a palm), and the event says whether the pad's
button is down (`button`).
**Each kind of touchpad has a palm model of its own** (`olo_palm::Kind`,
crates/palm/src/kind.rs; Ben, 2026-11-05: "each touchpad type should
have its own specific custom model type and weights and scoring/training
should adapt"): its own features, network size, base model trained on
real hands on that pad, decision timing, and training and judging rules
(a palm by size only where the pad reports one; a press by pressure or
by the pad's button). A model says its kind (`PalmModel::kind`; "" is a
MacBook's), and a person's trained one runs only on its own kind
(`Kind::model`); `score`, `train_from` and `lua` take a model's size from
the model. The MacBook's (`Kind::MacBook`) is as above. The ThinkPad's
(`Kind::ThinkPad`) goes by what its firmware says of a contact (now,
ever, how often, at once), its speed and force, and its age (8
features, 6 hidden units), judged from its first report; it is made by
`make_thinkpad_model` from Ben's captures in `crates/palm/data` (each
report as the kernel's slots stood, labelled by step; docs/TOUCHPAD.md
"The ThinkPad's touchpad"). The tracker keeps a palm the kernel ends and
brings back under a new id (within `AGAIN_US` and `AGAIN_MM`) one
contact. On a ThinkPad the filter (`THINKPADS`, its "thinkpad" profile,
`DeviceKind::Thinkpad`) keeps a palm a palm until it lifts: the
firmware's word held (libinput follows it both ways), or the model's
in its first `decide_ms`, told to libinput as the contact's tool type
(rewritten where the frame carries one, else in the room of events
libinput doesn't read there: the single-touch axes and the pressure);
its quirk lets pressure through with libinput not counting touches or
palms by it. Nothing else of the filter (hover, sizes, the click) runs
there. `a_thinkpads_palms_stay_palms` replays the captures through the
plugin as libinput would get them.
`SyntheticEvent::Contacts` stands in for the pad headless (a MacBook's,
or any `pad`, with its `button`). Preferences'
"Train for Your Palm" (apps/preferences/src/palm_trainer.rs) personalizes
the model: see docs/TOUCHPAD.md "The trainer".

Preferences ▸ Touchpad & Mouse goes into libinput (`configure_pointer`), with the
rest set as a Mac trackpad behaves: a two-finger tap is the secondary
click, no drag lock, no middle-click emulation. Scrolling has momentum, as
on a Mac (`momentum.rs`): when the fingers lift (libinput's scroll of 0)
while moving faster than 0.3 px/ms over the last 80 ms, the scroll glides
on, slowing by 0.997 a millisecond (`glide` stretches that: 2.0 glides
twice as far; `scroll_speed` scales finger scrolls before momentum
samples them); fingers resting again (a hold gesture), pointer motion, a
click, a swipe or pinch, or a wheel stop it. A pinch reaches the window
under the pointer only with Gestures' "Pinch to zoom" on
(`TouchpadSettings::pinch_zoom`, off by default: two fingers drifting
together while scrolling zoomed Chrome's pages); off, app_server keeps
the whole gesture to itself (`AnvilState::pinching`, decided as it
begins).

**A window shown gets its frames, idle or not.** Smithay sends a
surface its frame callbacks every frame only on its *primary scanout
output* (the screen showing most of it, from the last frame's render
states), else once a second. The DRM backend updates that only from a
frame that was drawn (`render_surface`): a frame with nothing new (an
idle screen) reports nothing as presented, and would take it from every
window there, so an idle Olo window (which scrolls in its frames) barely
scrolled under the wheel or the touchpad until something on its screen
drew. The headless backend renders each frame whole, so tests didn't see
it; Chromium draws on its own clock, so its windows didn't either.

**Scrolls go on the frame clock** (`pace.rs`). A touchpad reports at its
own rate, which beats against the display's: passed on as they came, one
frame got one report and the next two, and the content juddered though
the fingers moved evenly. So finger scrolls are held back and, once a
frame of the screen under the pointer (`pre_repaint`, before clients are
told to draw), app_server sends where the fingers were a report's time
ago (the interval it measures, 4–16 ms, plus 1.5 ms), interpolated
between the reports either side, as Android resamples touches. Each
report is placed by its own time, the kernel's on the pad's report
(`pace::reported`; one from another clock, or more than 50 ms old, is
taken as now), not by when app_server read it: read a few ms late (its
loop busy drawing), it was a short step then a long one (Logs at 1.5×
with the cores busy: 0.53–1.53× the fingers' speed, now 0.87–1.13). A
synthetic scroll's `at_us` is its time the same way: tools/dev/bench and
scroll-check stamp each report with when it was meant to be, so their
own lateness isn't measured as the app's. The glide
goes out the same way, from where the fingers left off, so every frame
gets exactly one scroll of one frame's worth and the hand-over into the
glide is seamless. The lift's stop goes once the fingers' scroll is all
out. Olo's apps (`scroll::Toolkit::Olo`: a process with an
`olo-menus-PID.sock`) get the glide as a scroll of its own, source
`continuous` (`Pacer::sending_glide`, focus.rs), which GPUI takes as
a glide (`ScrollKind::Glide`), so it bounces off an edge as on a Mac; everything else sees
one finger scroll whose stop is held back until the glide ends (GTK adds
no kinetic scroll of its own). Test with `tsession flick NAME DY` (ten
scroll steps, then the lift); `tsession scroll NAME 0 0` is a lift on its
own; `tools/dev/scroll-check` measures it frame by frame (Testing).

**Scrolling** goes the same distance in every program (`scroll.rs`, at
the one place app_server hands a scroll to a client): Olo's apps
scroll a finger scroll's values as logical pixels and a wheel notch 48
(the kit counts notches from `value120`, which wl_pointer 8 sends in
place of `discrete`; read only `discrete`, a notch came out as its 15°
angle, 15 px).
Chromium (Chrome, Chromium, Electron apps: found by `icudtl.dat` and a V8
snapshot beside the program) scrolls finger values 12× as far and a notch
120 px, so gets them ×1/12 and ×0.4 (value and value120); GTK 4 (a
process with `libgtk-4.so` mapped) scrolls finger values 2.5× as far (a
notch the same 48), so gets them ×0.4. GTK 3 (`libgtk-3.so`) scrolls a
tenth of either value (a notch's is 15) times its scrolled area's
height to the power ⅔ (`get_scroll_unit`): 448 px for a 100 px finger
scroll and 67 a notch in a 300 px view. app_server can't see the
scrolled area, so it goes by the window's height (`focus.rs` finds the
window from the surface): a finger's values × 10 / h^⅔, a notch's
× 32 / h^⅔ (its values, not `value120`, are what GTK 3 reads).
Measured with a PyGObject window of numbered lines that prints its
adjustment: 94–97 px for 100 and 45–47 a notch at 300 and 700 px, 1×
and 2× (the view a little shorter than the window). A wheel's notch is
scaled first by Preferences ▸ Touchpad & Mouse ▸ Mouse ▸ Wheel's
scrolling speed (`Settings::wheel_speed`, 0.25 to 3, `wheel_at_speed`
for libinput's notches and synthetic ones alike), its value and its
120ths together, so every program keeps the same step at any speed:
headless at 1× and 2× (a page reporting `scrollY` in its title, and
StyledEdit's view compared shot to shot), a notch moves Chrome 48, 96
and 24 CSS px and StyledEdit 48, 96 and 24 points at 1, 2 and 0.5.
`{"type":"frames"}`'s `wheel` says the last notch as it came (value,
120ths, the speed) and as it went (value, 120ths, to which kind of
program): a person's mouse whose notches feel short is read there. Chromium is recognised before
the kit's socket, since olo-chrome answers on one named after Chrome's
pid for its page trees. Chromium also flings on its own when a finger
scroll stops within 200 ms of its last move, however long the fingers
rested, where Olo's apps don't glide after a 50 ms rest; so a stop with
no glide waits 250 ms after the last move before it goes to a Chromium
window (`hold_stop`, input_handler.rs), or goes at once ahead of whatever
comes next (more scrolling, a click). Measured headless: a page whose
title shows `scrollY` with rows every 10 px (Chrome), and a numbered
file in gnome-text-editor (24 px lines); `tsession scroll NAME 0 N` (no
lift) and `tsession wheel NAME 1`. Against StyledEdit's `scroll-check`
(a 540 px drag, resting 150 ms before the lift: 540 px; a 347 px flick:
1140), Chrome goes 539 and 1137. Re-measure after a toolkit update.

## Night light (app_server night.rs)

Preferences ▸ Appearance's night light (off, on, or scheduled, at a
colour temperature) is resolved against the clock in
`desktop.rs::update_appearance`; `night.rs` fades the screens' white point
there over 1.5 s. The udev backend loads each CRTC's gamma ramps scaled
by the gains (`Backend::set_white_point`), so it costs no rendering and
screenshots stay neutral; new screens and a returning VT session get the
current gains. Displays with no gamma ramp but a colour matrix (Apple's
display controller on the MacBook) get a diagonal `CTM` instead, in a
non-blocking atomic commit of its own; one refused because a flip is in
flight is retried at that screen's next vblank, and app_server puts the
identity back when it exits (the matrix outlives it). At start-up the
gains are set at once, so a reload keeps the warmth without a flash. Headless logs each step ("white point") for
tests.

## Screenshots (app_server screenshot.rs, apps/screenshot)

`Request::Screenshot { path, output, window, border, pointer, format }` (only
`path` is required; `format: "rgba"` writes raw pixels instead, see
`olo_protocol::ImageFormat`, for live views) saves a PNG of one output (the one the pointer is on
by default) or of one window, and replies once the file is complete. It
becomes a `screenshot::Shot` for one output, which every backend takes
with that output's next frame (`screenshot::take`): the screen via
`render::capture`, or the window alone over transparency via
`render::offscreen` (its tab and border with `border`, keeping the tab's
shape, or only the client's area). A thread then draws the pointer in
(the cursor it is showing, from `cursor.rs`), unpremultiplies, encodes and
writes `NAME.part` and renames it, so readers never see half a file.
Window shots are refused while the screen is locked.

The Screenshot app (`olo-screenshot`) follows Haiku's: it takes a
picture as it starts, then shows it with Capture active window / Include
window border / Include mouse pointer / Delay, and Name / Save as (PNG,
JPEG, BMP, TIFF, WebP) / Save in (Home, Desktop, Pictures, Choose
folder…). New screenshot minimizes its own window first (so it isn't in
the picture and the active window is the one behind it) and brings it
back; Save writes `screenshotN` and quits, with the file's path on the
clipboard; Copy to clipboard offers `image/png`. Record… pops a menu:
Record Screen, Record Active Window (the one behind Screenshot's), Record
Region… (dragged out); its window steps aside (minimized, as for New
screenshot, `capture::step_aside`), the recorder starts on its own
(`--toggle-record`, the pointer as Include mouse pointer has it, telling
when it's saved) and Screenshot quits; with a recording going on this
display the menu is Pause (or Resume) and Stop Recording instead.
Settings are in `screenshot.json`.
On the GPUI kit (olo-ui): the preview is made on a worker at exactly
the frame's size in device pixels for the window's scale (again when
the scale changes) and drawn one pixel to a pixel; the form sits at
fixed rows so the caption under the picture can line up with one (a
picture whose caption would land a few points under a row's middle is
made that much smaller, `frame`). The buttons are a spacing below the
form's last row and the window as tall as that (`Layout::height`,
`fits_content`): a fixed 276-point height had let "Save in:" slip under
New screenshot and Save once the kit's rows grew. After
Save the window closes and the app holds the path on the clipboard
(`olo_kit_base::clipboard::hold`) until something else is copied, then
ends; GPUI's clipboard now offers images and data in their own MIME
types (`ClipboardItem::from_entries`, `new_data`). The picture
app_server writes for it (`olo-screenshot-PID.png` in the runtime
directory, memory) goes when it quits, and one left by a Screenshot
that was killed is swept at the next start. `--silent [--window [ID]]`
saves straight into the folder with no window: app_server runs it for ⌘⇧3
and Print Screen (the screen) and Alt+Print Screen (the shortcut's target
window by id: the active one, or the one the palette was opened over,
since the palette is active by the time it runs).
`--region` (⌘⇧4, region.rs) captures every output, covers each with an
exclusive overlay (a GPUI layer surface) showing its picture dimmed, one
pixel to a device pixel, and saves the rectangle you drag (its size in
pixels follows the corner); Escape or a click cancels. `--pick-color`
does the same undimmed, with a loupe by the pointer.
Saved with no window (`--silent`, `--region`), it prints the file's path
and puts it on the clipboard (`olo_ui::clipboard::hold`), staying until
something else is copied. ⌘⇧5 opens the app.

## Screen recording (apps/screenshot recording.rs, crates/video)

`olo-screenshot --record` records a display (or `--window ID`, or
`--display NAME`, or part of one: `--region X,Y,W,H` in points from its
top left, or `--region` alone to drag it out on the screen frozen as for
⌘⇧4, `region::pick_region`, Escape recording nothing) into an MP4, all
in process: frames from app_server's
capture (`olo_capture`, the same ext-image-copy-capture sessions as
screen sharing, a dmabuf where the GPU allows), H.264 from OpenH264 in
its screen-content mode (`olo_video::H264`; its non-real-time screen
mode is refused by this OpenH264, "Invalid usage type"), into an MP4 of
Olo's own writing (`olo_video::Mp4`: each sample written as it comes,
with its own duration, the index at the end; the shared writer Agent
Screens' Export Video uses too).

- **Only what changed.** A frame is asked for at most `--fps` times a
  second (30 by default, 60 at most) and comes only once the screen has
  changed; the recorder waits for it on the connection (`poll(2)` on its
  fd and on a pipe its signal handlers write to, `WAKE`, so a stop or
  pause wakes it whichever thread took the signal and one that came just
  before the wait isn't missed; no timer, but `--seconds` running out),
  and holds each picture until the next comes, so a still screen costs
  nothing and the video still keeps its time (the Viewer shows such a
  video's average, "13 fps"). Measured with `tools/dev/cost` at 1920 ×
  1080 on NVENC: a still screen or a paused recording, the recorder's
  own thread never wakes (it woke 4 times a second before the pipe);
  what wakes, 24 to 26 times a second at 0.0–0.1% of a core, is
  NVIDIA's driver's threads while its encoder is open. A screen
  changing every frame (a Terminal printing nonstop): 25% of a core
  (the encoder 16%, the capture 9%), 23 frames a second; a 960 × 540
  region of it, 11%, 28 a second.
- **Encoded on a worker** as frames come; one that comes while the last
  is still being encoded waits in its place, a newer taking it, on the
  most powerful encoder there is unless told (`--encoder auto|cpu|
  nvidia|vaapi`; `olo_video::Encoder`, `Choice`): NVIDIA's GPU where its
  encoder opens (`olo_video::nvenc`: NVENC's API through vendor/
  nvenc-sys's declarations, `libnvidia-encode.so.1` and `libcuda.so.1`
  opened at run time, nothing linked, so a machine without them builds,
  starts and records on the processor; a constant quality, as many
  bits as keep it, at most twice `olo_video::bitrate`, a key frame every
  two seconds, no B-frames, no lookahead (a picture out for each in),
  the parameter sets or sequence header with each key frame), else the
  processor (OpenH264); unless told, as Preferences ▸ Power ▸ Graphics ▸ Screen
  Recordings has it (`Settings::screen_recording.encoder`: the most
  powerful there is, named, by default; the NVIDIA GPU, offered where
  there is one; or the processor). A frame at 4K: 13.4 ms on the GPU (an RTX 50
  laptop part, `nvenc_makes_h264_that_decodes`, each frame decoded back
  by OpenH264), 111 ms on the processor (AVX2, one thread; 16 ms at
  1440 × 900, 56 at 2560 × 1600: `encode_cost`), so 4K keeps up at 30 or
  60 a second on the GPU and about 9 on the processor. NVENC's
  structures are filled from zeroed memory through raw pointers and read
  as Rust values only once every field is valid (an enum whose zero
  isn't one of its values set first, or filled by the driver). The JSON
  says which encoded it (`encoder`). A window that changes size is cut
  or shown on black at the size it began.
- **AV1 on Intel's and AMD's GPUs** (`--encoder vaapi`, Preferences ▸
  GPUs offering "Intel Graphics (AV1 only, VA-API)" where its driver
  encodes AV1; `olo_vaapi`, crates/vaapi): Fedora's own VA-API drivers
  (iHD, Mesa) leave out the patented codecs, so on the ThinkPad's Intel
  GPU (xe) they encode AV1 profile 0 and VP9 but no H.264 or HEVC: a
  recording through VA-API is AV1 (asked for anything else, an error;
  `--encoder auto` takes it for AV1 when there's no NVIDIA encoder, and
  first while the computer is on its battery, so the NVIDIA GPU isn't
  woken for it: `olo_video::on_battery`, a battery and no mains or USB-C
  supply online, a mouse's battery not counting).
  libva is opened at run time (`libva.so.2`, `libva-drm.so.2`; vendor/
  va-sys holds bindgen's declarations from libva 2.24's headers), the
  render node is the first whose driver offers `VAProfileAV1Profile0`
  on an encode entrypoint (NVIDIA's skipped, and so left asleep; asking
  every driver takes about 15 ms, which Preferences does on its reading
  thread). As FFmpeg's `av1_vaapi`: constant quality (CQP, quantizer
  `olo_vaapi::QUALITY`, 40), a key frame every two seconds then P
  frames each from the one before, on two reconstructed surfaces taken
  in turn; VA-API leaves AV1's headers to the program, so the sequence
  header (profile 0, 8 bits, 4:2:0, BT.709 limited, order hints, no
  CDEF, restoration or superres) and each frame header are written here
  (`Writer`, `frame_header`, `Tiles` for the tile layout AV1 allows:
  5K takes two columns) and handed over as packed headers, the driver
  writing the tile groups after them; the loop filter at a level from
  the quantizer, told to the driver as the header says it. Pictures go
  in as NV12 (BT.709, limited range) converted on a few threads straight
  into the input surface's memory (`vaDeriveImage`; else an image put
  in). Measured (`vaapi_sizes`, a 2560 × 1600 screen typed into, 3 s,
  luma PSNR of each picture decoded back by rav1d): quantizer 40, 515 KB
  at 47.6 dB (the processor's H.264 on the same: 160 KB at 41.9); 72,
  260 KB at 45.5; a frame in about 8.5 ms, conversion and upload
  included, a key frame up to 25. `vaapi_frames_come_back` encodes eight
  frames and decodes them back (each within 38 dB of what was sent).
- **Frames that never leave the GPU** (VA-API on the GPU app_server
  draws with: the Intel GPU on the ThinkPad, as on battery or `--encoder
  vaapi`): a display, or a window not on a background, is captured into
  three dmabufs of the capture's own in turn (`Capture::lend_frames`,
  `lend`: a frame lent out isn't drawn into again till its `Lent` is
  dropped; none free, it comes into the capture's own buffer and is
  copied), each imported once as a VA surface (DRM PRIME 2, kept by its
  key) and made NV12 straight into the input surface by VA-API's video
  processor (BT.709, full range in, limited out; `Av1::encode_prime`,
  `olo_video::Encoder::encode_prime`), the buffer given back as soon as
  that's done. Only in layouts VA-API reads (`olo_vaapi::imports`:
  linear, and Intel's X, Y and 4 tiles without compression; libva can't
  be asked, and Xe2's compressed 4, 0x0100000000000010, imports without
  complaint and reads as noise). Used when the capture's GPU is the
  encoder's (`Capture::render_device`, `Encoder::prime_device`); a frame
  smaller than the video (a window that shrank) is copied, one the
  encoder refuses dropped and the rest copied; `OLO_RECORDING_COPY=1`
  copies every frame. What it prints says how many went this way
  (`frames_on_gpu`). Measured (`tools/dev/cost`, release, a 2560 × 1600
  headless display, a Terminal printing nonstop, 29 frames a second):
  the recorder 2.3% of a core against 15.8% copying (the encoder's
  thread 1.9% against 9.6%, the capture's 0.4% against 6.2%), app_server
  4.1% either way. `vaapi_dmabuf_frames_come_back` encodes linear and
  tiled dmabufs, each drawn in again after its import, and decodes them
  back (54 dB). NVIDIA's encoder can't take them: app_server draws on
  the Intel GPU, and CUDA imports no other GPU's memory.
- **An encoder that fails mid-recording** is opened again once (its
  first picture a key frame), the stream going on in the same file.
  Failing again, the next encoder that makes the same codec takes over
  (`Encoder::others`, best first: NVIDIA's then the processor for H.264;
  NVIDIA's and the Intel GPU's for AV1, the Intel GPU first on battery;
  none for HEVC), and the recording says so: `encoder_note` in what it
  prints, and the saved notification's "An encoder failed; finished on
  …". Another encoder's parameter sets differ, so its stream goes in a
  sample entry of its own (`Mp4::new_entry` before its first picture,
  which must be a key frame): `stsd` lists each, `stsc` maps each run
  of samples to its entry, and an entry the same as the one before is
  joined to it (an encoder opened again usually makes the same sets).
  The readers follow: the Viewer gives each key frame its entry's sets
  (`media::mp4_later_entries`; the mp4 crate reads only the first
  entry), GIF export gives the decoder a later entry's sets at its first
  sample, and Trim copies every entry and switches at the same samples
  (`trim::video_entries`, which reads `moov` alone rather than the
  whole file). With no other encoder, or the file refusing (a full
  disk), the recording stops there and what was recorded is kept: the
  file finished, `stopped_early` saying why, the recorder stopped.
  `OLO_RECORDING_FAIL_AT=N` makes picture N fail (`N!`: the encoder
  opened again too, so the next takes over; `N!!`: every other too, so
  it stops): checked headless with `--encoder nvidia` and `30!` (NVENC
  to picture 30, then OpenH264, 33 frames; the Viewer seeking past the
  switch, a GIF of it, a trim across it). Tests: `olo-video`'s
  `another_encoders_stream_gets_its_own_sample_entry` (the file's
  structure, joining, Trim) and `a_recording_goes_on_with_another_encoder`
  (NVENC then OpenH264, every picture decoded); `olo-preview`'s ignored
  `plays_across_another_encoders_entry` (`OLO_FALLBACK_MP4`,
  `OLO_FALLBACK_SEEKS`).
- **H.264, HEVC or AV1** (`--codec`, `Settings::screen_recording.codec`,
  Preferences ▸ Power ▸ Graphics ▸ Format, which offers H.264 and HEVC;
  `olo_video::Codec`): HEVC and AV1 on NVIDIA's encoder only (the
  processor makes H.264: `--encoder auto` falling back to it says so in
  `codec`). The MP4 gets the codec's sample entry made from the stream
  (`olo_video::config`): `avc1`/`avcC`, `hvc1`/`hvcC` (profile, tier,
  level, chroma format and bit depths read from the SPS, emulation
  prevention taken out), `av01`/`av1C` (profile, level, tier and colour
  layout read from the sequence header OBU, which goes in as its
  configOBUs); parameter sets, sequence headers, access unit and
  temporal delimiters and padding are kept out of the samples. Each
  codec's quality was chosen by measuring (`codec_sizes`, a 1280 × 800
  screen typed into, 3 s; luma PSNR of each picture decoded back, and
  bytes): the bitrate H.264 had spent 277 KB for 48.6 dB; at constant
  quality 20 it keeps 48.4 dB in 190 KB; HEVC at 26, 48.9 dB in 105 KB
  on preset P6 (used up to 2560 × 1600; 4K on P6 took 29 ms a frame, so
  P4 there: 145 KB); AV1 at 28, about 220 KB: NVIDIA's AV1 lacks the
  screen content tools, so it isn't smaller on screens and the panel
  doesn't offer it. Scrolling the same: H.264 249 KB (the bitrate's
  319), HEVC 186. A real recording (5 s, typing, 1280 × 800): H.264 171
  KB (262 before), HEVC 96, AV1 180. A 4K frame on P4: H.264 14.6 ms,
  HEVC 13.9, AV1 10.9 (`nvenc_cost_by_codec`, the copy into NVENC's
  buffer included). Trimming and GIFs take all three (the sample entry
  copied whole; HEVC and AV1 decoded by `olo_images`, one thread so
  each sample's picture comes out at once), and so do the Viewer and
  `olo-screen video-frame`.
- **Sound** (`sound.rs`, `--sound computer|microphone|both|none`, else
  `Settings::screen_recording.sound`, the computer's by default; Record…'s
  ticks in Screenshot, "Record the Computer's Sound" and "Record the
  Microphone", set it): what the computer plays (the default output's
  monitor, `olo_system::recording::COMPUTER`) and the microphone,
  recorded over the sound server's native protocol
  (`recording::Capture`, float stereo at 48 kHz, a piece every 40 ms on
  the client's thread), mixed on a thread of its own woken by each piece
  (a stream a second behind, or not yet started, filled with silence;
  each stream's first sound lined up with the video's start), encoded
  as AAC-LC at 160 kbit/s by libfdk-aac (`olo_video::aac`, opened at run
  time as NVENC is: Fedora's `fdk-aac-free`), its access units handed to
  the encoder worker, which writes them into the MP4 beside the pictures
  (`Mp4::with_sound`, `add_sound`: a second track, `mp4a` with its
  `esds`, an edit list cutting the encoder's priming). Paused, the sound
  is dropped too. No sound server or no encoder: the video without
  sound, the JSON saying why (`sound` null, `sound_note`). Checked
  headless with `olo-fakesound`, whose record streams play 440 Hz from
  the microphone and 660 Hz from the monitor: both tones in the file,
  decoded by symphonia as the Viewer does, the sound as long as the
  video (3.008 s for 3.001; paused 2 of 6 s, 4.01 for 4.0; over 12 s
  the stand-in, paced by sleeps, falls 0.1 s behind real time). The
  mixer is woken once every stream has four frames (85 ms: the sound
  only goes into a file), 12 times a second. Measured (`tools/dev/cost`,
  a still 1920 × 1080 screen on NVENC): 0.4% of a core without sound,
  2.4% with both, the AAC encoding 0.5% of it, the rest the two
  streams' client threads, woken for each piece (every 10 ms from the
  stand-in; a real server's are 40 ms).
- **Clicks and shortcuts shown** (app_server `showing.rs`,
  `Request::SetShowing { clicks, keys }`): a ring spreading from where a
  button is pressed (an agent's synthetic clicks too) and the chord just
  pressed on a key cap low on the screen, written as the keyboard
  profile writes it (`Chord::label`: ⇧⌘4 on the Mac profile), from the
  physical keys (`Keymap::chord_now`), before the profile maps them for
  apps. Only chords with ⌃, ⌥ or ⌘ and a few keys alone (Return,
  Escape, Tab, Delete, the arrows): never typing, so a password typed
  while recording never shows. Drawn over everything but the pointer
  (see-through: no opaque region), so recordings and screen shares
  have them; the displays stay awake only while a ring spreads (450 ms)
  or a key cap fades (1.6 s). The recorder turns them on for a
  recording that asks (`--show-clicks`, `--show-keys`, or the settings:
  Record… ▸ Show Clicks, Show Shortcuts) and off after.
- **A countdown** (`--countdown N`; ⌘⇧6 and Screenshot's Record… count
  down `ScreenRecordingSettings::countdown`, 3 s; agents' recordings
  none): N, N-1, … 1 on a square key cap in the middle of the screen
  under the pointer, a second each (`Request::Countdown`, said by the
  recorder as each second begins; a stop ends it), while the encoder
  opens; the recording begins after both.
- **The encoder opened first**: NVENC takes 2.5 s to open (its CUDA
  context), and while it did, the frames that came only replaced each
  other: every GPU recording's first seconds were one picture. The
  worker now opens the encoder at the size the capture gives (a
  region's part of it) before the recording's clock starts, and the
  recorder waits for it (`ready`); the sound's description reaches the
  file once the sound has started.
- **Exporting with edits** (`olo_video::export`; `olo-screenshot
  --export VIDEO [--to PATH] [--keep A-B,…] [--cut A-B,…] [--crop
  X,Y,W,H] [--mute] [--format mp4|gif] [--width N] [--fps N] [--quality
  high|medium|low] [--encoder …] [--codec …] [--progress]`): a recording
  made again with what's kept (a trim is one stretch, each cut splits
  one), each picture cropped (its pixels) and made smaller (area
  averages, the crop's shape kept, never larger, even), at most so many
  pictures a second, as finely as asked (`Quality`: the recording's own,
  or about 60% and 35% of its bits: NVENC's quality level up 4 and 8,
  OpenH264's bitrate, VA-API's AV1 quantizer up 30 and 60), its sound
  kept or left out, its marks kept where their moments are (`marks_of`
  reads `chpl`). The pictures come in order from `read::Video` (the
  reader `gif.rs` had, now shared: `each_picture` gives each with when
  it shows from and till); each is shown for as long as it was in
  what's kept (`kept_time` collapses what's cut, so a picture across a
  cut joins up with itself, one wholly cut is left out), its times put
  on the frame rate's grid when there is one. The sound (AAC) is
  decoded by symphonia (its priming dropped as its edit list says), the
  stretches kept taken to the sample, and coded again by libfdk-aac (a
  test cuts half of each of a 440 Hz and an 880 Hz second: a second
  comes out, 440 then 880). A GIF is made of the edited video
  (`gif::from_mp4`). On the caller's worker, `progress` told 0 to 1,
  `cancel` checked between any two pictures (SIGINT or SIGTERM for the
  command), written as `PATH.part` and put in place when whole. A 3 s
  1280 × 800 recording cut, cropped and made 320 wide: 2.6 s, most of it
  NVENC waking.
- **Trimming** (`olo_video::trim`; the Viewer's Edit ▸ Trim…, ⌘T;
  `olo-screenshot --trim VIDEO --start S --end E [--to PATH]`): nothing
  encoded again. The pictures are copied from the key frame at or
  before the start to the one showing at the end (the last cut short),
  an edit list starting playback exactly at the start (`Mp4::start_at`);
  the sound (AAC) from the frame before the one holding the start (the
  decoder's overlap) to the one holding the end, its edit list cutting
  what's before (the source's own read from its boxes, `edit_starts`:
  the mp4 crate doesn't give it). A video with reordered frames
  (B-frames) is refused. Olo's player honours the edit lists: a trimmed
  copy at 0 s is pixel for pixel the original at its start. In the
  Viewer the player's timeline gets a handle at each end of what's kept
  (`PreviewView::set_trimming`; what's cut dimmed; dragging one shows
  the picture there; its two handles are what agents see of the bar
  then), Cancel and Trim over the picture; Trim writes "NAME
  (trimmed).mp4" beside it on a worker and shows it.
- **GIFs** (`olo_video::gif`): `--as-gif` (or the settings' `gif`,
  Record… ▸ Save as GIF, which dims the sound ticks) makes the recording
  a GIF when it's done, the MP4 gone; `olo-screenshot --gif VIDEO
  [--to PATH] [--fps N] [--width N]` makes any of Olo's recordings one.
  The video decoded by OpenH264 in two passes: the first trains one
  palette for the whole GIF (NeuQuant, 255 colours and a transparent
  one, on 16 pictures from across it, so colours don't shift between
  frames), the second writes a frame for each change at 10 frames a
  second (a picture held longer is one frame, longer: delays counted in
  hundredths from the start, so they don't drift), each only the
  rectangle that changed with what stayed the same transparent, at most
  960 pixels wide by area averages. Colours are looked up by their 15
  bits, cached. Written as `.gif.part`, then put in place; SIGINT and
  SIGTERM stop it as they stop a recording. A 4 s recording of
  Terminal at 1280 × 800: 11 frames, 960 × 600, 294 KB.
- **The pointer in a recording** (app_server `showing::captured_pointer`,
  `render::captured_pointer`; `Request::SetShowing`'s `pointer_size` and
  `smooth_pointer`): `--pointer-size K` (1 to 4; Record… ▸ Larger
  Pointer is 1.5) draws it K times its size in what's captured, the
  built-in cursor drawn at that size by `olo_hvif` (never enlarged; a
  client's own cursor is stood in for by it then, as screenshots do);
  `--smooth-pointer` (Record… ▸ Smooth Pointer) draws it gliding after
  the real one, a critically damped spring (rate 18 a second: a jump 95%
  done in 0.26 s, never past it), solved exactly for each frame's time so
  uneven frames glide the same: a hand's jitter smoothed, an agent's
  jumps made moves. Only captures have it: the screen's pointer is as it
  was. While it glides the displays draw on (`keeps_awake`), so the
  captures have the glide; arrived, nothing. A window's recording now
  has the pointer too, where it is over the window's content
  (`render::content_origin`, as screenshots and the IPC find it).
  Measured (`tools/dev/cost` on app_server, a headless 2× screen
  recorded, 4 s): the pointer still, 0.5% of a core and 12 wakeups a
  second, smoothed or not; jumping twice a second, 0.8% plain and 2.5%
  smoothed (the frames of the glides, 100 wakeups a second).
- **The camera bubble** (Screenshot's `bubble.rs`, `olo_camera`;
  `--camera [ID]`, Record… ▸ Show Camera, `ScreenRecordingSettings::camera`
  "" / "auto" / a node): the camera in a circle 176 points across, low in
  the screen's right corner (24 points in), a ring in the panel's colour
  and a soft shadow, over everything. Its own process
  (`olo-screenshot --camera-bubble`, a kit layer window on the overlay
  layer), started by the recorder before the countdown (so you see
  yourself while it counts) and ended after: it says `ready` and the
  camera's name once its first frame is up, or why not (then the video
  is made without, `camera_note` saying why). On the screen, so a
  recording of the screen or a region has it. A window's recording is
  only the window, so on a background (`--background`) the recorder
  opens the camera itself and draws it on each frame (`inset.rs`): a
  disc a fifth of the video's shorter side (96 to 360 pixels) on the
  background's bottom right corner over the window's, ringed as the
  bubble is, the latest frame scaled to it once on a worker as it comes;
  what it covers outside the window is kept from when the background
  was made and put back each frame, so its soft edge never piles up. A
  window alone has none (`camera_note` says so). Dragged, it moves: its layer
  surface's margins (GPUI's new `Window::set_layer_margin`), kept on the
  screen. Frames on a worker: the camera's middle square mirrored (as a
  mirror shows you), only the latest waiting for the window (a
  `RenderImage` each, the last dropped). Agents read it as an Image named
  "Camera: NAME". Measured with the stand-in (`tools/dev/cost`, 4 s): the
  window 30 frames a second at 374 µs each; the process 7.2% of a core,
  5.2% of it the stand-in drawing its pattern (a real camera's MJPEG
  decoding instead).
- **Captions** (`olo-screenshot --caption TEXT [--seconds N]`,
  `olo-screen caption-video`; `Request::Caption`, app_server
  `showing::caption_elements`): an agent's narration, drawn by app_server
  into captures only (a screen's, unzoomed over a zoom, and a window's,
  low on the window's picture), never on the screen: white on a dark
  panel 7% from the bottom, at most four fifths of the picture wide,
  wrapped to three lines (the last cut with an ellipsis), fading in and
  out in 180 ms, for N seconds or till the next (an empty TEXT takes it
  down). Made once for its scale and width; while it fades the displays
  draw on only if a capture waits for a frame, and a timer wakes them
  when it ends.
- **Marks** (`olo-screenshot --mark TITLE`, `olo-screen mark-video`):
  the moment marked, timed as the video is (pauses left out: the title
  left beside the going file, SIGRTMIN+1, the recorder taking them at
  its next look), written as chapters (Nero's `chpl` in `moov`'s `udta`:
  FFmpeg, mpv and VLC read it; `Mp4::chapters`) and listed in what it
  prints (`marks`, seconds and titles). The Viewer shows a video's
  chapters as ticks on its timeline and in Go (Previous and Next
  Chapter, and each by name: it plays from there); agents read the one
  playing in the Position slider's value.
- **Zoom** (app_server `zoom.rs`; `SetShowing`'s `zoom`, `Request::ZoomTo`):
  `--zoom-follow` (Record… ▸ Zoom on Clicks; a whole display's recording
  only, not a region's or a window's) zooms the recording in twice
  around each click, pans to keep the pointer 15% inside the view, holds
  while clicks and keys come, and zooms out three seconds after the last
  (a calloop timer wakes the displays then, put off while the hold is
  extended). `olo-screenshot --zoom-to X,Y,W,H|out` (Agent Screens:
  `olo-screen zoom-video NAME X,Y,W,H|out`) zooms to that part of the
  display, as much as shows all of it (at most 8×), and holds it till
  `out`. The view (centre and magnification) glides on a critically
  damped spring (rate 7 a second: 95% there in 0.68 s), solved exactly
  for each frame's time. It's drawn by the GPU as the capture is: the
  display's elements rescaled and moved as a mirror's are (`render::frame`
  with a `Fit`), so the recorder does nothing more; the shortcut's key
  cap is drawn apart, unzoomed in its place (`showing::apart`), so a
  shortcut shows wherever the view is. Only capture sessions
  (`capture_elements`' `zoomable`): screenshots and the screen are as
  they are (a screen share of that display while recording is zoomed
  too). While it glides the displays draw on, only while a capture waits
  for a frame (`keeps_awake`; the pointer's glide likewise), so a
  recorder that died can't keep them drawing. Measured (`tools/dev/cost`
  on a headless 2× session's app_server, 6 s with three clicks): 0.7% of
  a core plain, 1.0% zooming; idle after, 0.2%.
- **A window on a background** (`backdrop.rs`; `--background`, the
  settings' `background`, Record… ▸ Window on a Background; only for a
  window): each frame is the window, its corners rounded (10 points'
  radius, anti-aliased against what's under them), on the wallpaper
  blurred and dimmed to 86%, a soft shadow under it (a rounded
  rectangle's distance, dropped a quarter of its spread), with a margin
  of 6% of its longer side (32 to 128 pixels, less where the video would
  pass 4096, what the encoders take); without a wallpaper, a gradient of
  the desktop's colour. The background is made once, as the encoder
  opens (from a pack picture's preview: it's blurred at an eighth
  anyway; enlarged in one fixed-point pass on a few threads: 24 ms for
  a 3840 × 2216 video, where `image`'s resize took 232 ms); the shadow
  and what shows through the corners again only when the window
  changes size (it's centred again: the video's size can't change); and
  the canvas keeps them between frames, so a frame copies only the
  window's rows: 1.9 ms at 4K, where cutting and padding a window
  (`fit`) took 2.1. A window grown past the margin is cut.
- **A region** is cut from each frame of its display as it comes (the
  display copied whole into a buffer kept for the next, then the rows
  of the part), so the encoder sees only the part: 400 × 300 points at
  1.5× is a 600 × 450 video.
- **Pauses**: `olo-screenshot --pause` (SIGUSR1) and `--resume`
  (SIGUSR2). Paused, nothing is asked for (a frame on its way is let
  go) and the time paused is left out of the video's clock, so it goes
  straight from the moment before to the moment after (a 4 s recording
  with 2 s paused is 2 s long); `--seconds` counts what's recorded. The
  recording's file says it (`paused`), and Deskbar's record light shows
  a pause sign in its ring (two bars on device pixels, as far apart as
  they are wide), its menu offering Resume Recording for Pause
  Recording.
- **Ends** on `olo-screenshot --stop` (SIGTERM to the recorder, which
  finishes the file; `--stop` waits for the recording's file in the
  runtime folder to go, which the recorder removes once what it made is
  whole, a GIF from it too, by inotify, or for the recorder to end,
  whichever comes first: one started by the shortcut lives on a while
  after to say where its file went, and `--stop` used to wait 30 s for
  it; now 0.13 s; then it prints the path), `--seconds N`, SIGINT, or the window closing. Written as
  `.mp4.part` until whole, then into the Videos folder as "Screen
  Recording 2026-11-04 at 18.30.05.mp4" (or `--to PATH`); it prints what
  it made as JSON (`path`, `seconds`, `frames`, `width`, `height`). One
  recording at a time on each display: what's going is in
  `$XDG_RUNTIME_DIR/olo-recording-<display>.json`
  (`olo-screenshot --recording` says it), so an Agent Screen's and the
  person's don't meet.
- **A region's shape and size** (`region.rs`, the screenshot's region
  picker too): a bar at the top of each screen while it's picked, as
  CleanShot X's: Free, 16:9, 4:3, 1:1 (keys 1 to 4; ⇧ makes a free drag
  square) held while the corner is dragged (`keep_shape`: the longer
  way the pointer went sets the size); 1280 × 720 and 1920 × 1080 (keys
  7 and 8) place a frame that many pixels big in the middle (as large as
  fits), which a press in it drags (kept on its screen) and Return or a
  click in it takes. Presses on the bar are its buttons', never a drag.
  Checked headless: a 16:9 drag saved 356 × 200 (1.78), a placed
  1280 × 720 moved and taken saved 1280 × 720; `review --all` finds
  nothing on the bar.
- **Quiet while recording**: Notifications treats a recording going on
  its display as it does a share (its `while_sharing`, Preferences ▸
  Notifications ▸ "While sharing or recording the screen"): no banners
  (errors wait too) from the moment it records to its end, those on
  screen put in the history; the recording's file is watched with the
  sharing's. Record ▸ Countdown sets the seconds counted down first
  (None, 3, 5, 10; the settings' `screen_recording.countdown`).
- **For people**: ⌘⇧6 (Ctrl+Alt+Print with the Windows and BeOS
  profiles; `Shortcut::ScreenRecording`, "Record the screen, or stop")
  runs `olo-screenshot --toggle-record`: a recording of the screen
  starts, or the one going finishes. While it records, Deskbar's tray
  has a red record light (the kit's `style::recording_light`, from the
  recording's file in the runtime folder, by inotify:
  `olo_protocol::recording`), its tip and menu saying since when, with
  what the controls below have and Stop Recording
  (`olo-screenshot --stop`). Finished from the shortcut, it tells in a
  notification ("Screen recording saved", its name, length and size)
  with Show in Tracker, the file selected in its folder.
- **The controls on the screen** (`controls.rs`, `olo-screenshot
  --controls`, which `--toggle-record` starts beside the recorder, and
  reaps): a bar at the top of the screen, as CleanShot X's, with the
  record light, how long it has recorded (pauses left out: the
  recording's file carries its clock, `recorded_ms` as of `as_of`), and
  Pause (Resume), Mark, Restart, Discard and Stop; while one is being
  done, or during the countdown, what's happening takes the row. A layer
  surface in `olo_protocol::recording::CONTROLS_NAMESPACE`, which
  app_server leaves out of a capture session's pictures (a recording, a
  screen share: `capture_elements`' `zoomable`, its `CAPTURING`), never a
  screenshot's (an agent's look sees what the person does): on the
  screen, never in the video. A floating bar with room round it for its
  shadow (so its parts sit tool-bar tight in it); never takes the
  keyboard. It follows the recording's file (watched; its clock ticks on
  the second while it records, not at all while paused) and goes when
  the recording does. Discard (`olo-screenshot --discard`: a flag beside
  the recording's file, then SIGTERM; the recorder deletes what it made
  and says nothing) and Restart (`--restart`: discarded, then the
  recorder's own arguments started again, kept in the recording's file
  as `args`, a region that was dragged out given as `--region X,Y,W,H
  --display NAME` so it isn't asked for again). Checked headless at 1×,
  1.5× and 2×, light and dark (`tsession review --all`: nothing found);
  each button clicked; a frame of the video decoded where the bar was
  shows the wallpaper.
- **After** (`after.rs`, `olo-screenshot --after PATH`, which
  `--toggle-record` starts once the file is whole, in the saved
  notification's place; the notification only when it can't start, or
  when the recording has something to say: an encoder that failed, a
  recording stopped early): a card in the screen's lower left, as
  CleanShot X's quick-access overlay, on the Top layer (never in front of
  a full-screen app's Overlay) and never taking the keyboard. The
  recording's picture (a tenth of the way in, at most a second: past a
  countdown; decoded by `olo_preview::video::picture_at` on a worker,
  made at the window's scale with `preview_of`, one pixel to a device
  pixel, again when the scale changes), its name, then its length, its
  size in pixels and its bytes, and Show in Tracker (`tracker::open`,
  the file selected), Copy (the file as file managers copy one,
  `clipboard::files`, served by `clipboard::hold` with no window until
  something else is copied) and Delete (to the freedesktop Trash, which
  Tracker restores from), Edit (the recording in the Viewer's editor,
  `olo-viewer --edit`, below), with a close box. The picture pressed and
  moved 4 points drags the file out (`dnd::files`: a folder copies it, a
  chat or a mail attaches it); dropped somewhere, the card goes. It goes
  by itself 20 s after the pointer left it, the window asked then
  whether the pointer is over it (`is_window_hovered`: a drag out of it
  takes the pointer without the leaving being told), and stays while it
  is. Checked headless at 1×, 1.5× and 2×, light and dark (`tsession
  review --all`: nothing found); dragged into a Tracker window (the file
  copied there), Copy pasted as the path, Delete in the Trash, Show in
  Tracker its folder; idle, 0 wakeups.
- **Recordings** (`library.rs`, `olo-screenshot --library`; Record ▸
  Recordings…, Deskbar's recording menu; one window, a second
  `--library` brings it forward): every screen recording in the Videos
  folder, as CleanShot X's history, in the kit's BColumnListView: its
  picture (a tenth in, at most a second, at the row's size for the
  window's scale, one pixel a device pixel), name, length ("GIF" for a
  GIF), size and date, newest first (a header click sorts), found by
  name as it's typed ("1 of 4 recordings"). Play and a double click (the
  Viewer), Edit (the Viewer's editor), Show in Tracker, Rename… (a
  dialog; the extension kept, a name taken refused) and Delete (to the
  Trash). What each is and its picture are read on a worker, 64 at a
  time, as they're needed, and kept; the folder is followed (inotify),
  so a recording made, renamed or deleted anywhere shows at once. A
  recording is known by Olo's mark, not its name: the recorder, the trim
  and the export write a box of Olo's own in the MP4's `udta` (`oloR`,
  `Mp4::mark_olo`), a GIF a comment (`olo_video::made_by_olo` reads
  either), so one renamed stays; files named "Screen Recording …" (from
  before the mark) are listed too. `olo-screenshot --recordings` prints
  the list as JSON for agents (path, name, seconds, width, height,
  bytes, modified). Checked headless at 1×, 1.5× and 2×, light and dark
  (`tsession review`: no errors); searched, renamed (still listed),
  deleted (in the Trash, gone from the list at once).
- **Editing** (the Viewer's `video_edit.rs`: Edit ▸ Edit Video…, ⌘E;
  `olo-viewer --edit PATH`, the card's Edit, which a running Viewer is
  handed): CleanShot X's editor, under the playing video (the window
  grown by it, within its screen). A timeline: pictures along it (48
  made in one pass by `olo_video::read::Video::pictures_near`, at the
  timeline's height for the window's scale, each tile the one made
  nearest its middle, one pixel a device pixel; made again when the
  scale changes), the sound's waveform (`olo_video::export::peaks`, the
  AAC decoded on a worker; greyed when muted), the marks as flags with
  their titles (before the flag where they'd pass the end), a handle at
  each end of what's kept, the cuts dimmed, the playhead. A press on it
  plays from there; a drag picks a stretch; a handle dragged trims,
  the picture there shown. Its bar: Cut (the stretch picked, or the
  mark chosen; Delete too), Add Mark (at the playhead, "Mark N"), Crop
  (the Viewer's crop frame, `crop.rs`, dragged over the video's
  picture: `PreviewView::video_picture` says where it was drawn; Crop
  again or Return takes it, Escape puts it away; the crop chosen shows
  as what goes dimmed), Mute, what's kept ("0:10 kept of 0:12") or how
  the export goes, Export…, Done. Every step can be undone and done
  again (Edit ▸ Undo, Redo); playing, what's cut is skipped and what's
  past the trim stops it (paused: the player stops only to let the
  video go). Export… asks Format (MP4 or GIF), Size (whole, three
  quarters, half, a quarter of the crop), Frame rate (as recorded, 60
  down to 10) and Quality, then makes "NAME (edited).mp4" (or .gif,
  "edited 2" if that's taken) beside it on a worker
  (`olo_video::export`), its percentage and Cancel in the bar, then
  "Exported …" and Show in Tracker. Done with edits not exported asks
  first. Nothing changes the video itself. Agents read the timeline
  (`Slider` Timeline, Trim Start, Trim End; each mark a `Button`; the
  cuts, the stretch picked and the crop as labels) and drive the same
  through `olo-screenshot --export`. Checked headless at 1×, 1.5× and 2×,
  light and dark (`tsession review`: nothing found); a 12 s video with
  sound and two marks cut, marked, cropped, muted and exported at half
  size: 200 × 130 (half the 400 × 260 crop), no sound, the marks moved
  back by the cut.
- **For agents**, on their screen: `olo-screen record-video NAME
  [--seconds N] [--fps N] [--window ID] [--region X,Y,W,H]
  [--no-pointer] [--to PATH]` starts it on the screen's display and
  prints the path at once; `pause-video` and `resume-video` pause it,
  `stop-video` finishes it. `olo-screen video-frame VIDEO [OUT.png]
  [--at SECONDS]` writes the picture a video shows at a time
  (`olo_preview::video::picture_at`, decoded as the Viewer seeks), so an
  agent checks what it recorded without playing it. A screen started at
  4K (`--size 1920x1080 --scale 2`) records at 3840 × 2160: checked, its
  video played back in the Viewer on that screen.

## Screen sharing (apps/portal, app_server capture.rs)

Apps never capture the screen themselves on Wayland: Chrome, Slack,
Firefox and OBS ask xdg-desktop-portal's ScreenCast, which asks a
backend to show a picker and hand back PipeWire streams. Olo's backend is
**olo-portal** (`olo-portal --service`, run by olo-session in a real
session): it owns `org.freedesktop.impl.portal.desktop.olo`; olo-session
describes it to the portal (`~/.local/share/xdg-desktop-portal/portals/
olo.portal`) and routes ScreenCast (and the others below) to it in `olo-portals.conf`
(`graphical.rs`), restarting xdg-desktop-portal when those change. It
runs out of app_server on purpose: a share outlives an app_server reload
(the stream's capture reconnects, `stream::reconnect`; the app sees a
pause of a fraction of a second), libpipewire stays out of the
compositor, and an upgrade waits while it shares or asks
(`olo_ui::upgrade_waits_while`).

No interface falls to another desktop's backends. `olo-portals.conf`
names every one the portal has: Olo's for what olo-portal serves, the
keyring for Secret, and `none` for the rest (Lockdown, Clipboard,
InputCapture, Usb, DynamicLauncher: the portal says they aren't there),
with `default=none`. xdg-desktop-portal reads the configuration of every
desktop `XDG_CURRENT_DESKTOP` names, though, and the session's is
`Olo:GNOME` (for Qt, "Other toolkits' programs" below): what Olo's left out went to
GNOME's (`gnome-portals.conf`: `default=gnome;gtk`), and Settings was
gathered from GNOME's and GTK's backends as well as Olo's, which kept
xdg-desktop-portal-gnome, xdg-desktop-portal-gtk and
gnome-control-center (GNOME's shortcuts dialog, under GNOME's portal)
running. So while an Olo session runs the portal service alone is told
the desktop is Olo: a drop-in in the runtime unit folder,
`$XDG_RUNTIME_DIR/systemd/user/xdg-desktop-portal.service.d/olo.conf`
(`Environment=XDG_CURRENT_DESKTOP=Olo`, `graphical::PORTAL_DROP_IN`),
which `stop` removes and the end of the login takes with the runtime
folder; an agent screen's frontend is started with the same
(`own_bus::PORTAL_FRONTEND_ENV`). Its log (`xdg-desktop-portal -v`)
then shows only Olo's, `none` and the keyring's.

- **Settings** (`settings.rs`, `org.freedesktop.impl.portal.Settings`,
  version 2): what sandboxed programs, and GTK 4, libadwaita, Chrome and
  Firefox where they ask the portal, read for the desktop's look:
  `org.freedesktop.appearance` (`color-scheme` 0 light or 1 dark,
  `accent-color` GNOME's blue as (r, g, b), `contrast` 0) and, under
  GNOME's names, the same list app_server writes to GSettings for
  unsandboxed programs (`olo_theme::toolkit::desktop_settings`: the
  colour scheme, GTK theme, accent, fonts and their hinting, icon and
  pointer themes, `button-layout`), so the two can't disagree.
  `ReadAll` takes the portal's patterns (a trailing `*`); `Read` says
  `org.freedesktop.portal.Error.NotFound` for what it hasn't. It follows
  app_server's settings (`settings::follow`, events, asked again when
  app_server comes back) and signals `SettingChanged` only for what
  changed (light to dark: the two colour schemes and the GTK theme).
- **Notification** (`notification.rs`, version 1): a program's
  notification handed to Olo's centre as a `Notify` (its title, body,
  `priority` as urgency, a themed icon by name or a picture's bytes as
  `image-path`, kept in `$XDG_RUNTIME_DIR/olo-portal-pictures` by its
  contents; `desktop-entry` its app id; `default-action` as the
  `default` action and each button as `button-N`), the centre's number
  kept with the program's own id (the same id again replaces it; removed
  by `CloseNotification`). The centre's `ActionInvoked` comes back as
  the portal's, with the action and target the program gave
  (`notification::follow`); `NotificationClosed` forgets it. Tried on a
  test bus: a notification sent to the frontend shows in the centre with
  its button, and invoking the button gives the program `app.open` and
  its target.

Its dialogs are on the GPUI kit, each over the window of the app that
asked (`WindowOptions::foreign_parent`, xdg-foreign: the handle the app
exported), each its own view answering its request once (`ui::Answer`):
a dialog that goes without an answer (closed from its tab, cancelled by
the app through its `Request`, which closes the window) answers no as
its view is released. The `Portal` entity keeps each dialog's window by
its request, for the app's Cancel and for holding an upgrade off.

- **D-Bus** (`portal.rs`): CreateSession serves an
  `impl.portal.Session` at the handle the portal chose (Close drops its
  streams); SelectSources keeps the cursor mode (hidden, drawn in, or
  apart as metadata: below) and
  a restore token (`("olo", 1, connector)`); Start shares without asking
  when the token names a screen still there, or when the same app asked
  less than 5 s ago (Chromium opens several sessions for one share), and
  otherwise asks the picker, while an `impl.portal.Request` at the
  request's handle lets the app cancel it. (Chrome opens a session for
  its own dialog's preview and another once its Share is pressed,
  passing the first's restore token: so one picker.) It answers with each stream's
  PipeWire node, size, connector or `window:ID` (`id`) and, for a
  screen, its position. Screens and windows (`AvailableSourceTypes` 3);
  the picker offers what the app asked for (`types`).
- **The picker** (`picker.rs`): a fixed dialog over the asking
  window: who asks (from the app id; "An app" when the
  portal can't tell, as for unsandboxed apps), the screens and the
  windows (not minimized; under Screens and Windows headings when both
  are offered, the windows scrolling past two rows) as `ThumbnailPicker`s
  (radio buttons to agents) of live thumbnails, and Cancel / Share
  (Enter, Escape, or a double click). Screens' wells take the first
  screen's shape (192×108 for 16:9), windows' are 160×100, all whole
  device pixels at every quarter scale; a worker keeps a capture of each,
  asks for a frame now and then (a screen twice a second, a window every
  two; app_server answers when it changed), into shared memory
  (`Capture::in_memory`: a dmabuf mapped for reading in the portal's own
  GPU-drawing process came back black, every thumbnail an empty well;
  app_server reads a frame back for it, 1.0% of a core while the picker
  is open at 1280×800), and shrinks it to the well's
  size in device pixels for the scale it is drawn at, only ever from a
  whole frame (`Capture::frame_size`: not a buffer allocated for a new
  size that waits for its first, which is uninitialised memory), read
  into one buffer reused for every frame (the worker's heap is given
  back as it ends, `malloc_trim`: a screen's frame is 16 MB). Each
  thumbnail replaced leaves the GPU's atlas (`cx.drop_image`), and the
  last ones as the picker goes: two a second otherwise filled it, 325 MB
  after ten seconds.
- **The stream** (`stream.rs`): one thread per share runs PipeWire's
  loop with app_server's connection as one of its sources. The node is
  its own driver offering BGRx at the screen's size, up to 60 frames a
  second; while it streams, a frame is asked for at most at the rate the
  app agreed to, and app_server answers once the screen changed (a still
  screen sends nothing). It offers two formats, dmabufs first: BGRx
  with the modifiers app_server draws (`Capture::dma_modifiers`, a
  choice marked `DONT_FIXATE`), then in memory. An app that takes
  dmabufs (Chrome, OBS, GStreamer) says which of those it can import;
  the portal picks of them what gbm allocates best, offers that alone,
  and once it's agreed every one of the stream's buffers is a dmabuf of
  the portal's (`ALLOC_BUFFERS`, made in `add_buffer`) that app_server
  captures straight into (`Capture::request_into`, damaged whole, as
  the buffers take turns): a frame goes from the screen to the app on
  the GPU, never read back. An app that takes only memory gets memfds
  the frame is copied into from the capture's own buffer. Each frame
  carries a header (`SPA_META_Header`: when it was taken on the
  monotonic clock, and its number). A screen or window that changes
  size is offered again at its new size (new buffers). A frame that came
  as one app left goes to the next. `olo-portal read-test NODE OUT.png
  [--dmabuf|--linear]` reads a node as an app does (dmabufs read back
  through gbm), and `share-test` takes the same flags.
- **Capture** (app_server's `capture.rs`; the client is the
  `olo-capture` crate, shared with Agent Screens' live view,
  `olo_capture::live`): the portal is a client of
  app_server's ext-image-copy-capture on the direct socket
  (`olo-$WAYLAND_DISPLAY-direct`: olo-wlproxy hides the protocol from
  outside programs). It allocates a dmabuf with gbm on the render node
  app_server names, in a modifier the GPU likes (reading it back is a
  detiling blit, ~7 ms at 5K, where a linear one costs 30–40 ms of
  uncached reads), falling back to shared memory. In app_server each
  session is a `Cast` in `State::captures`: a frame it asks for waits for
  its output's next frame (`Captures::take`, every backend) and is drawn
  only when its own damage tracker says the picture changed since its
  last (the first comes at once); a dmabuf is drawn into on the GPU (only
  what changed, when it is the buffer drawn last) and told ready when its
  fence signals (`Captures::finish`), so app_server's loop spends 0.26 ms
  median, 0.57 ms p95 a frame at 5120×2880 with the pointer moving; a
  shared-memory buffer is drawn into the session's own texture and read
  back as BGRA rows (a few ms at 5K: the fallback). Constraints follow the
  screen's rotation and are sent again when it changes size; the pointer
  is drawn in when the session asks (`paint_cursors`).
- **Windows** (`capture::Target::Window`): each window has a handle in
  the foreign-toplevel list whose identifier is `olo-ID`
  (`AnvilState::sync_toplevel_handles`, as the window list changes), so
  the portal finds a window Olo's IPC named and makes a capture source of
  it. Its frames are taken with the frame of the output it is mostly on:
  only what its client draws (no tab or border, `render::window_picture`),
  whole even behind another tab of its group, at that screen's scale;
  its size is the client's, and the stream is offered again when it
  resizes. One on another workspace, or hidden, is drawn with the frame
  of the display it's parked on, from what it showed last (it draws
  nothing new while unseen), so the share picker's thumbnail shows it
  and a share of it starts (`capture_windows_on`, `capture_window`).
  Nothing about shares is looked at in a frame unless one waits for a
  frame (`Captures::pending`). Closing it stops its sessions, and the share ends two
  seconds later (`Capture::stopped`): app_server stops every session as
  it goes too (a reload, an upgrade), so a stop is only an end if the
  connection outlives it; else the share waits for app_server and
  carries on, its dmabufs made known to the new connection. While
  the screen is locked a window's share pauses on its last frame. Both
  protocols are kept from outside programs (wlproxy's list doesn't have
  them). A window share reconnects after a reload by its id, or one with
  the same app id and title (windows come back with new ids).

- **Zoom**: its own app offers Wayland sharing only when its
  `XDG_CURRENT_DESKTOP` is GNOME, so Olo starts it so
  (`olo_xdg::quirks`, from every launch path; the portal picks its
  backends by its own environment, so Olo's still answer). Zoom in
  Chrome needs nothing.
- **Agent screens as sources**: on the person's own display the picker
  offers each agent screen running (`olo_kit_base::moving::screens`,
  under Screens as "Agent Screen “NAME”"), captured from that screen's
  own app_server (`olo_capture::Source::Display`, its Wayland display),
  so an agent's screen can be shared into a call. Tried in a private
  world: chosen in the person's picker, its stream through PipeWire
  carried the screen's 1440×900 desktop.
- **Shown as shared** (app_server's `private.rs`): nothing is kept out
  of a share or a recording (Ben, 2026-11-06: "nothing should be
  intentionally blacked out"). Passwords' windows, prompts and the lock
  screen show as they are, in an agent screen's Live view too (which
  showed Passwords as a black box: it's a capture session of the
  screen's app_server). A window being
  shared has a ring round its body on the person's screen (a
  recording's red, two points, laid out in device pixels as the
  decoration lays out its frame, so flush outside its border at every
  scale), drawn with the display's frame and never into what's shared:
  the portal tells app_server which windows its shares are of
  (`Request::SharedWindows`, as it publishes `SharingState`, whose
  shares carry `windows` too). Checked with `olo-portal capture-test`
  (a screen and a window share of Passwords, both showing it), the ring
  at 1, 1.25, 1.5, 1.75 and 2×, and end to end on an agent screen: a
  window shared through its portal's picker ringed, the ring gone with
  the share.
- **Screenshots** (`shot.rs`): the Screenshot portal has Olo's
  Screenshot app take the picture (`olo-screenshot --silent --for-app`,
  or `--region` when the app wants the person to choose; `--for-app`
  only prints the path, leaving the clipboard alone), so it lands where
  ⌘⇧3's do, as the person set it, and its `file://` URI goes back.
  PickColor isn't offered yet.
- **Permission dialogs** (`access.rs`): the Access portal, which the
  others ask through (xdg-desktop-portal only offers Screenshot when
  there is one): a BAlert over the asking window with the portal's icon
  where Olo's theme has one (else the padlock), its title, subtitle and
  body, its choices (a checkbox for a yes/no, a pop-up menu for several)
  and its deny and grant buttons (Escape denies).
- **Open and Save** (`files.rs`): the FileChooser portal's OpenFile,
  SaveFile and SaveFiles as the GPUI kit's file panel
  (`file_panel::show_in`, in `file_panel::window_options` over the
  asking window), titled as the app asks, starting where that app's
  panel was last (`places::last_for`, opening and saving apart, kept in
  `places.json` as the panel goes, `left_in`, Cancel included; by the
  app id the portal gives, else the app whose window has the keyboard),
  else in its `current_folder`; a save naming a `current_file` starts
  in that file's folder. It proposes the app's `current_name`. The app's filters (globs and MIME types; `current_filter`
  shown first, or added) are the panel's Show pop-up, `multiple` lets it
  choose several files, and its `choices` sit above the buttons; the
  answer carries the `uris`, the filter shown and the choices made.
  Chrome's and Electron's uploads (`<input type=file multiple
  accept=…>`) come here by themselves, as do every Flatpak's; host GTK
  programs only when asked (`GTK_USE_PORTAL=1` for GTK 3's
  FileChooserNative, `GDK_DEBUG=portals` for GTK 4's FileDialog, both
  tried), which Olo doesn't set for everything: they also send GTK's
  link opening through the OpenURI portal.
- **Open With** (`chooser.rs`): the AppChooser portal, which
  xdg-desktop-portal's OpenURI asks when an app opens a file or link
  whose handler isn't settled (a Flatpak opening a document, a type with
  several apps, `ask`): as BeOS's Open With window, what is opened
  ("Open “Report.pdf” with:", a `file:` link by its file's name), a list
  of the apps offered (icon, name, GenericName), the last choice
  selected and the keyboard in the list; Open, Enter or a double-click
  answers with the app's desktop id, Cancel or Escape with none. The
  default (the `last_choice` the portal gives) is first; Olo's own apps
  (`olo.viewer`, see `open::for_others`) show by Olo's names and icons.
  UpdateChoices replaces the list live, the selection kept. The desktop
  entries are read off the UI thread, on the bus side.
  xdg-desktop-portal launches the app and counts the choice: for a host
  app it asks until the same app was picked 3 times for the type, which
  is why Olo doesn't point every GTK program at the portals (see Open
  and Save). To try it, route AppChooser to olo in a test config and
  give the test x-d-p an XDG_DATA_HOME whose `applications` has a
  harmless entry (`Exec=/usr/bin/true %u`) and a `mimeinfo.cache` (GIO
  associates types only through that), then ask OpenURI for a custom
  scheme.
- **Picking a colour**: the Screenshot portal's PickColor runs
  `olo-screenshot --pick-color` (`region.rs`): the screens frozen as for a
  region, undimmed, with a loupe below right of the pointer (above or
  left at the edges): 9×9 of the pixels around it at 10 points each,
  cells on device pixels, the one under it ringed in black or white by
  its lightness, and its swatch and hex below. A click prints "R G B",
  which goes back as the portal's `color` (fractions); Escape cancels.
- **Desktop background** (`wallpaper.rs`): the Wallpaper portal's
  SetWallpaperURI. When the app asks for a preview, a BAlert asks first
  ("Set as Desktop Background?", the picture fitted under it, a texel to
  a device pixel at most); the picture is copied into
  `~/.local/share/olo/backgrounds` (so it stays when the app's copy
  goes) and set as app_server's `Settings::wallpaper`, as Preferences ▸
  Backgrounds would.
- **Who you are** (`account.rs`): the Account portal's
  GetUserInformation asks in the permission dialog ("Share your name and
  picture with Slack?", your full name and user name, the program's
  reason; Share / Don't Share), with your picture as its icon
  (AccountsService's, else `~/.face`; the dialog draws a picture file
  given as its icon, `widgets::alert_icon_rect`); shared, it answers
  `id`, `name` and `image` (a file URI). The Camera and Location portals
  need nothing of Olo's own: xdg-desktop-portal asks them through the
  Access dialog.
- **Email** (`email.rs`): ComposeEmail becomes a `mailto:` link (the
  addresses, copies, subject and body, encoded as RFC 6068 says, line
  breaks as CRLF) opened in the person's mail program through
  `olo_xdg::open`; a link can't carry files, so attachments are shown in
  Tracker, to drag in.
- **Print** (`print.rs`): PreparePrint shows Olo's Print panel
  (`olo_print::panel::view`, the `gpui` feature's, over the asking
  window) and answers with GTK's
  print settings (printer, `n-copies`, `page-ranges` counted from 0, or
  `output-file-format` pdf) and page setup (the paper in millimetres,
  its PWG name) and a token; Print takes the program's document from
  the file descriptor (a PDF, or PostScript by its `%!PS`) and, for the
  token's choice (or one asked now), sends it to CUPS over IPP with its
  own format, or asks where to save it (Olo's Save panel, `files::Ask`).
  Checked on a test session's bus with busctl: PreparePrint, then Print
  of a PDF through a descriptor saved byte for byte.
- **Global shortcuts** (`shortcuts.rs`, app_server `grabs.rs`): the
  GlobalShortcuts portal (Discord's push-to-talk, a browser extension's
  keys). An app's triggers ("CTRL+SHIFT+a", xkb names) become Olo
  chords (`trigger`). The first BindShortcuts for an app asks, as a
  BAlert listing each shortcut and its keys ("Keys in use" for ones Olo
  has or another app took, found by trying them; "No keys" where none
  were suggested); what was allowed is kept per app in
  `$XDG_CONFIG_HOME/olo/global-shortcuts.json`, so the same shortcuts
  bind again without asking. A session holds an app_server connection
  that took the keys (`Request::GrabKeys`): app_server matches them after
  its own shortcuts and sends their presses and releases to that
  connection alone (`Event::KeyGrab`; not to the focused window, not
  while locked; a plain letter or the like can't be taken, a function
  key can), which the portal passes on as Activated and Deactivated;
  closing the connection lets them go. When app_server restarts (an
  upgrade) the portal takes them again from the next one; closing the
  session (or binding again) lets them go first.
- **Running in the background** (`background.rs`): the Background
  portal's backend. xdg-desktop-portal asks the person first (through
  Access, Olo's permission dialog), then here to write or remove the
  app's login item (`~/.config/autostart/APP.desktop`, `X-Flatpak`, the
  command line quoted as the spec says; an id with a `/` is refused),
  which olo-session starts at login; GetAppState says which apps have
  windows (running) and which has the focus (active), from app_server,
  so it can tell which run with none; one found so is allowed to.
- **Keeping awake** (`inhibit.rs`): olo-portal also answers the Inhibit
  portal (an inhibition of idle or suspend lasts until the portal closes
  its request) and serves `org.freedesktop.ScreenSaver` (Inhibit gives a
  cookie, held until UnInhibit or the caller leaves the bus), each held
  by an app_server connection that asked `KeepAwake` (see "Power").
- **The cursor as metadata** (the third cursor mode, 4: what remote
  desktops and OBS ask for, to draw the pointer themselves): the picture
  is captured without it, and the capture follows the pointer apart, on
  a cursor session of ext-image-copy-capture
  (`Capture::follow_cursor`: a `wl_pointer`, the session's position and
  hotspot events, and a capture session of its own for the pointer's
  picture, told apart from the screen's by its user data). app_server
  serves those sessions for screens (`cursor_capture_constraints`: a
  square `capture::CURSOR_BOX`, 32 points, at the screen's scale, shared
  memory): at each frame of a screen with a session on it, where the
  pointer is on it (its hotspot, in the screen's pixels; none off it or
  hidden) and its hotspot (`Captures::cursors`), and a cursor frame
  asked for drawn when the pointer's picture changed
  (`render::cursor_picture`, its own damage tracker; the first at
  once). Nothing runs without a session. The stream asks for
  `SPA_META_Cursor` with its buffers (room for a 256-pixel picture) and
  fills it on every buffer: the position and hotspot, and the picture
  (BGRA) when it changed since last sent; off the screen, an empty one,
  once. A move with nothing else changed is a buffer with no picture
  (its chunk's size 0), no more often than frames may come. Tried
  headless (`olo-portal stream-test - SECS --cursor metadata`, then
  `olo-portal cursor-test NODE SECS`, which asks for the metadata and
  prints it, in `tools/dev/pw-sandbox`'s PipeWire).
- **While sharing** olo-portal writes what is shared
  (`olo_protocol::SharingState`: its pid, and each session's app,
  screens and start, in `$XDG_RUNTIME_DIR/olo-sharing-<display>.json`,
  gone when nothing is; a file whose portal died counts for nothing),
  and Deskbar's tray shows a screen with a recording's red dot; its menu
  says what is shared with whom and has Stop Sharing for each (and Stop
  All), which runs `olo-portal --stop SESSION`: the portal ends the
  session and sends `Closed`, so the app stops too. A share whose
  stream ends by itself (app_server never came back) ends its session
  the same way.

To try it: `olo-portal capture-test OUT.png` (a frame, saying whether it
came as a dmabuf), `olo-portal stream-test - 10` (a PipeWire node for
ten seconds; `capture-test OUT.png window:ID` a window), and `olo-portal share-test OUT.png [--hold SECS] [--types 1|2|3]` (asks
xdg-desktop-portal for a screen as an app does: the picker comes up,
then it writes the first frame of the stream, read through the PipeWire
connection the portal gave, and keeps reading frames that long or until
the portal closes the session, saying when the format changes). In tests keep all of it private: a PipeWire
and WirePlumber of their own (`PIPEWIRE_RUNTIME_DIR` in /var/tmp,
WirePlumber's device monitors turned off), and xdg-desktop-portal,
xdg-permission-store and `olo-portal --bus ADDRESS` on the test
session's own bus (`XDG_DESKTOP_PORTAL_DIR` pointing at a folder with
`olo.portal` and `olo-portals.conf`; the test bus can't start services,
so the document portal it insists on needs a stand-in that answers
`GetMountPoint`).

### Remote control (apps/portal remote.rs, app_server remote.rs)

A program may control the computer through the RemoteDesktop portal,
once the person says yes: Chrome Remote Desktop, a KVM such as
Deskflow, a video call's remote control, an agent. olo-portal serves
`org.freedesktop.impl.portal.RemoteDesktop` (version 2, keyboard,
pointer and touch screen) beside ScreenCast, on the same sessions
(`portal::Shared`): CreateSession, SelectDevices (what it wants to
control; all, unless it says), ScreenCast's SelectSources if it also
wants to see screens, and Start, which asks the person: the picker,
titled "Share and Control Your Screen", saying what it will control,
its button Allow; or, for control alone, the permission dialog ("Allow
Deskflow to control this computer?"). Never remembered, never answered
by a restore token or an earlier choice: asked every time.

Its input comes one of two ways, as the portal says:

- **The Notify calls** (one event each) become app_server's synthetic
  input over its IPC, as an agent's does (`SyntheticEvent::MoveBy`, kept
  on the screens; `MoveOn`, a point in a window's content; `Keysym`, the
  layout's key for it with Shift if that's how it's typed, else typed on
  a keymap of its own; `Touch` with a window). An absolute position on a
  stream goes where that stream's screen is on the desktop, or into its
  window's content; on an agent's screen, nowhere. Each checks the
  session was started and granted that device.
- **ConnectToEIS**: a libei connection. app_server listens on a socket
  of its own (`olo_protocol::eis_socket`, `olo-<display>-eis`, mode 0600)
  that nothing announces: no `LIBEI_SOCKET` for every program to find,
  as the Smithay example it came from had. olo-portal asks for a grant
  (`Request::RemoteControl { session, devices }`, refused to anyone but
  olo-portal), connects, and hands the program that socket. app_server
  takes a connection only from the process that holds a grant (its
  `SO_PEERCRED` pid; a grant lapses after 10 s), and gives its seat the
  granted devices only: a keyboard on the session's layout, with a text
  device; a pointer and an absolute pointer whose regions are the
  displays (`mapping_id` the connector, as the screen shares' streams
  name theirs), updated as the displays change; a touch screen. Its
  events become synthetic input too, on every backend. After
  ConnectToEIS the session's Notify calls are refused.

While a session controls the computer, Deskbar's sharing item shows it
("Deskflow is controlling your keyboard and pointer", or "Zoom sees
Studio Display and controls your pointer"), with Stop Remote Control.
The session ending (the program closing it, Stop, its stream lost)
tells app_server (`Request::RemoteControlEnd`), which closes the libei
connection ("The remote control session ended.") and lets go of keys
left down.

To try it: `cargo run -p olo-portal --example remote_test -- unix:path=BUS
[HOLD]` against `olo-portal --bus unix:path=BUS` in a test session (a
`dbus-daemon --session --address=unix:path=BUS` of its own): it asks
for the keyboard and pointer (answer the dialog in the session), types
"hi" and moves the pointer 100 points right with the Notify calls, then
types "x" and moves it 50 down over libei, and holds the connection
until it's closed (Stop) or HOLD seconds pass.

## Viewer (apps/viewer)

The Viewer opens any file Quick Look previews (`olo-viewer FILE…`; one
process, a second start hands its files over: `instance::forward`), on
the GPUI kit. Each window is a `Doc` entity (its file, the files beside
it for Go ▸ Previous / Next File and the arrows, a picture's edits);
the `Viewer` entity keeps them, the signatures, the folder watch and the
panels. The document has the keyboard from the moment its window opens
(given it in the window's build, before the `Frame` would take it).

- Everything but PDF is `olo_preview::ui::PreviewView` (olo-preview's
  `gpui` feature): pictures in the kit's `ImageView` (zoom ⌘= ⌘- ⌘0 ⌘9,
  pan), formatted documents in `RichView`, sheets in `TableView`, and
  text, listings, fonts, sound and video. The window opens at the size
  its content asks (`ideal_size`) for the display it opens on.
- A video's picture (View menu, `PreviewView::set_video_zoom`,
  `VideoZoom`): Actual Size (⌘0, as before: at most a frame pixel a
  device pixel, smaller to fit), Zoom to Fit (⌘9, as big as fits) or
  Zoom to Fill (⇧⌘9, filling its place, what overflows cut off by a
  content mask), its proportions kept every way. Enter Full Screen (F,
  pictures too; Escape or F leaves): the picture is all there is (the
  menu bar rendered at no height, so its keys still work; the title and
  facts gone), Actual Size as big as fits, and the transport over its
  foot while the pointer moved in the last 2.5 s or it's paused (one
  timer, re-armed as it fires, hides it; a click on the picture plays
  and pauses then). Leaving full screen, app_server gives a window its
  place and its own size back (`WindowMeta::before_fullscreen`; asked
  for none, GPUI took nearly the whole display). Tried headless with a
  640 × 360 clip in a 700 × 700 window and a 12 s one full screen at 2×:
  the three ways, the transport gone 3 s after the pointer stopped while
  playing, and 700 × 600 at (100, 80) back after Escape; the layout
  review clean at 1×, 1.5× and 2×.
- A video or a sound showing: ← and → move its time by 10 s
  (`PreviewView::skip`, `SKIP_SECS`, held at its start and end), ↑ and
  ↓ go to the next file; Space or P plays and pauses. Quick Look's keys
  are the same (Tracker's `quicklook.rs`, and Viewer following a panel,
  `follow_keys`, leaves ← and → to it). Tried headless with a clip of
  Olo's own making (`cargo run -p olo-video --example test_clip OUT.mp4`:
  each second its own colour): paused at second 4, → 14, → 24, ← 14,
  the same file.
- Pictures: Image ▸ Rotate, Flip, Crop… (⌘K: `crop.rs`'s `CropFrame`,
  an overlay pinned over the preview with `.inset_0()`, its pointer's
  shape set as it paints; drag out a frame, move it or its edges, Return
  crops, Escape cancels) and Adjust Size… (a child dialog), steps undone
  and redone and saved by `olo_preview::edit`'s worker, a spinner while
  it works.
- PDF (`pdfdoc.rs` the model, `pdfwork.rs` the worker, `pdfview.rs` the
  view): pages rendered by hayro on the worker in 1024-pixel tiles for
  the zoom and scale, at most 12 MB uploaded a frame, the pages near the
  view kept. Forms are filled in place (a `TextField` over the field in
  the form's font; Tab and ⇧Tab between fields, Escape ends; check boxes
  and radio buttons by a click; a pop-up's choices from a menu under it),
  text boxes and ink added with the Text and Draw tools, signatures drawn
  once (New Signature…, `signature.rs`) and placed from Sign…; saved as
  an incremental update (earlier revisions and signatures survive). The
  toolbar's page and zoom are read as it paints: the page view settles
  them in its prepaint, after the toolbar was built. Agents read the
  fields as `TextInput`, `CheckBox` (checked), `RadioButton` (its
  on-state as its value), `ComboBox` or `ListBox`; while one is typed in,
  as its editor.
  Its text (`pdftext.rs`): read natively on the worker, once a page,
  after the page is first rendered (renders first; Select All and Find
  ask for the rest, `Request::AllText`, read a page at a time between
  renders), by running hayro's interpreter with a `Device` of our own
  that keeps each glyph's Unicode (ToUnicode, else its glyph name) and
  box (`props.transform * glyph.transform()`, 1000 units to the em, the
  em's -0.25 to 0.85) in the page's rendered points instead of painting;
  OCR'd scans' invisible text counts, ligatures paste as letters. Glyphs
  stay in the order the page draws them; between two, a space where the
  gap along the baseline is over a tenth of an em (a justified line's
  word space is a fifth; kerning far under a tenth) or a drawn space
  was, a line break where the baseline moves over half an em (or back
  more than two), a paragraph past 1.8. A line ending in a hyphen that
  the next goes on from in lower case is one word, pasted whole
  (`DIS-CLAIMED` keeps its hyphen). 1 to 2 ms a page (bzip2's manual).
  With the Select tool, a drag over text selects it (where no field or
  mark takes the press; an I-beam over the text's lines), a double-click
  a word, a triple a line, ⇧-click extends; across pages too. Drawn over
  the page's picture (`style::selection_over`, the kit's highlight,
  lighter than behind a text field's text). Edit ▸ Copy (⌘C; a field
  being typed in keeps its own), Select All (⌘A: copied once every page
  is read), Find… (⌘F: a bar over the pages, as the tool bar is, with
  how many it found and Previous, Next and Close; Return and ⇧Return;
  each match marked, the one shown selected and scrolled into view),
  Find Next / Previous (⌘G, ⇧⌘G), and a right-click's Copy, Select All
  and Find…. Agents read each line in view as a `Label`. Measured on
  release builds scrolling bzip2's manual at 60 frames a second: 457 to
  479 µs a frame before, 459 to 492 after; idle, nothing.
- A text or a formatted document (Markdown, Word, OpenDocument, RTF):
  Edit ▸ Copy, Select All, Find…, Find Next and Previous, as for PDFs,
  through the preview's (`PreviewView::copy`, `select_all`, `open_find`,
  `find_step`).
- `--look FILE` (a file panel's Space): a process of its own, put away
  by Space or Escape; `--over=HANDLE` opens it over that window (the
  panel's, exported: xdg-foreign).
- The picture dragged where it doesn't pan carries its file out; files
  dropped on a window open.
- A playing video keeps the screen awake (`Window::set_keep_awake`); it
  lets go when paused or played to its end (the preview asks for another
  frame when its clock stops as it draws, since the window read it
  before).
- Upgrades: the open files come back (`on_upgrade`, `restored_state`);
  the new version waits while a document has unsaved changes or is being
  saved (`upgrade_waits_while`), so nothing is lost.

## DeskCalc (apps/deskcalc)

An expression line over a keypad that scales with the window: R5's 5×4
keys, Haiku's scientific rows above them, or none; the context menu
switches keypads, angle units and copies/pastes. `expr.rs` is the whole
calculator (lexer, recursive descent, `format` for 15 significant digits)
and is unit-tested; keep new functions in `FUNCTIONS` and on the keypad
together. After =, a digit starts afresh and an operator carries on.
Settings, the last expression and history are in `deskcalc.json`.
The first app on the GPUI kit (`olo-ui`): the keypad is drawn in one
`canvas` (each key's bounds kept for the pointer, each said to agents
as a `Button` named by its key), the display is a `style::sunken` well
holding a right-aligned `TextField` that keeps the keyboard (Enter, ↑,
↓ and Escape are actions bound in the `DeskCalc` key context, which
the field's keys pass through); typed characters flash their keys.
Measured against the egui version (release, 2×): 57 MB against 83 MB,
42 frames for 20 clicks against 210, each 0.85 ms against 0.6 ms.

## StyledEdit (apps/styled-edit)

Plain text files with styles, as on BeOS: the text is the file; fonts,
sizes, colours, bold/italic/underline/strikeout (runs of byte lengths,
JSON) go in the `user.olo.styles` attribute, alignment and wrapping in
`user.olo.alignment` / `user.olo.wrap`, so other programs see plain
text. Styles that no longer fit the text (edited elsewhere) are dropped.
Non-UTF-8 files open as Latin-1 and save as UTF-8 (the status line says
so); unwritable files are read-only (selectable, not editable). Saving
writes `.NAME.saving` beside the file, copies its permissions and renames
it over the original, on a thread.

`doc.rs` is the model and is unit-tested. The app is on GPUI (the
GPUI kit, `olo-ui`): each window's `DocView` keeps the GPUI kit's
`editor::Editor` (so a document of any length scrolls and types at full
speed) and is its `EditorHost`, and the `Doc` is its `Document` (`Styled`
in main.rs, which also names each style's font, `fonts::font`): every edit is `Doc::edit`, which moves the
runs: inserted text takes the pending typing style, the style of what it
replaced, or the style before it, and pasting what was copied here
brings its styles back. Undo is our own (snapshots, typing and deleting
coalesced within 2 s); ⌘Z/⌘⇧Z/⌘Y are consumed before the editor sees
them, and what they change goes to the editor with `replaced`. Replace
All splices each match so the styles between them survive. A read-only
document's Font, Paragraph and Document menus are disabled
(`Menu::all_enabled`): its styles are saved with it.

Paragraphs (`doc::Para`, one per paragraph, kept with `para_starts` by
`Doc::splice`): a kind (`Kind`: Body, Title, Heading 1–3, Quote, Code;
choosing one restyles its text, `Kind::apply`, and gives it its space
above and below), a list (bulleted, numbered: `Doc::number` counts its
place among those at its level, deeper ones under it not breaking the
count), an indent level and its own alignment (the Paragraph menu's
Align items set the selected paragraphs'; the document's alignment,
`user.olo.alignment`, is only the default now). Removing text across
paragraphs leaves the first's setting; each newline put in starts a
paragraph set like the one it split, and Return at a heading's end
starts body text. In a list, Return in an empty item steps out a level
(ending the list at the outermost), Tab and ⇧Tab indent it more or less,
Backspace at its start takes its marker away (`Document::key`, which
the editor asks before it acts on those keys). `Doc::format` turns a paragraph into the
editor's `Paragraph` (24 points an indent level, and for a list's
marker, so list text lines up with a quotation's). They are saved in
`user.olo.paragraphs` (JSON, dropped if the count no longer fits the
text), undone with the text, and kept through a reload. The status line
counts words (in the selection too), for documents up to a megabyte.

Tables (`table.rs`): a paragraph's `Para::row` (`doc::Row`: column
shares, header) makes it a table's row, its cells its text between tabs,
so the file itself has the table as tab-separated lines. The Table menu
inserts one (3 by 3, a header row, after the caret's paragraph or in
its place if empty, with a paragraph after it if it would end the
document), converts paragraphs to rows and a table back to text,
toggles the header row, adds rows above or below and columns before or
after, deletes a row, a column or the table, and evens the columns,
each one step to undo (`Doc::replace_paras`: the text change, then the
new paragraphs' settings). Removing text across paragraphs gives the
paragraphs whose newlines are put back their own settings again
(`Doc::splice`), so rows stay rows. Markdown's tables come in as rows
(the head a bold header row), and Word, OpenDocument and RTF tables
too; Export writes them as GitHub tables, `<table>`s, or tables in the
other formats; printing sets each row's cells in their columns, ruled
and kept whole on a page. Columns dragged wider or narrower (a rule,
in the editor) keep their widths in StyledEdit's own files and in Word,
OpenDocument, RTF and HTML (`<colgroup>`); Markdown has no widths.

The ruler (Document ▸ Show Ruler, while lines wrap; each window keeps
its own through a reload, new ones take the last choice) is the GPUI
kit's `widgets::Ruler`, over the text (`Editor::text_span` places it,
and `EditorHost::text_moved` moves it with the text in the same frame): it shows the caret's
paragraph's indents and tab stops, and what is changed on it goes to
every selected paragraph (not a table's rows) as one step to undo, with
a line down the text while a marker is held. `Para::left`, `first` and
`right` are points beyond what the paragraph's kind, level and list
give (`Para::base_indents`; the ruler shows the sums), and `Para::tabs`
its stops (`doc::Tab`), after which a stop every half inch; Code
paragraphs without stops of their own keep a stop every four spaces.
Word, OpenDocument and RTF keep them all (`w:ind`/`w:tabs`;
`fo:margin-left`, `fo:text-indent`, `style:tab-stops` measured from the
indent as LibreOffice has them; `\li`, `\fi`, `\ri`, `\tx` with
`\tqc`/`\tqr`/`\tqdec`; checked in LibreOffice), HTML the indents
(tab stops it hasn't), Markdown neither.

File ▸ Print… (⌘P) shows `olo_print`'s Print panel; Export ▸ PDF… and
the panel's PDF File ask where, then write a PDF. `pages.rs` sets the
document on pages on a worker (with GPUI's text system made for it,
`TextSystem::standalone`, and the editor's own layout, so lines break
as in the editor): each paragraph as `Doc::format` sets it, at the
page's text width (an inch's margins), its rows down the pages (a row
that doesn't fit starts the next page; space above isn't kept at a
page's top), each word placed where the layout put it in its own font,
size and colour (justified text stays justified), list markers in the
indent, page numbers at the foot of documents of more than one page.

Pictures (Edit ▸ Insert Picture…, a picture pasted, or a picture file
dropped on the text) are characters: U+FFFC (`editor::PICTURE`) whose
style holds the picture (`doc::Picture`: its file, its size in points,
what it shows), so they flow with the text, are typed around, deleted,
copied, dragged and undone like any letter, and any number go in a
paragraph. `Doc::sections` lays the character out as big as its picture
(`editor::push_picture`: unseen, the row as tall and the glyph as wide),
and the kit's editor draws the picture over it
(`editor::Paragraph::pictures`), a frame while it loads, tinted when
selected. `pictures.rs` decodes each file on a worker (`olo_images`, so
JPEG XL, AVIF and HEIC too, no bigger than 2048 pixels a side) into a
texture. A picture put in is its pixels at 96 to the inch, no wider than
a page's text; Edit ▸ Picture Size fits it to the page or makes it
larger or smaller. In memory a picture's path is absolute; saved, it is
from the document's folder when it's in it, and a picture with no file
of its own (pasted, or from an imported document, held meanwhile in
`~/.cache/olo/styled-edit/pictures`) is copied into a folder beside the
document ("Report pictures" for "Report.txt") as it's saved. Word,
OpenDocument and RTF take each as a picture paragraph of its own
(`olo_richtext::Picture`: a drawing in `word/media`, a frame in
`Pictures/`, a `\pict`; GIF, WebP and the rest made PNGs), and give
theirs back the same way, checked in LibreOffice; a web page holds its
pictures (`data:` addresses), Markdown points at their files
(`![what it shows](path)`, read back from beside the file), and printing
and PDFs place each where its character is (`olo_print::Item::Image`).

Document ▸ Page View (per window, kept through a reload; lines wrap
in it) sets the text on sheets of the paper it prints on
(`olo_print::local_paper`) with the margins it prints with, breaking as
printing breaks (`editor::Pages`); each sheet shows its header and
footer as printed (`pages::margin_lines`), the ruler spans its text.

Document ▸ Header & Footer… sets what every printed page shows in its
margins (`doc::Page`, `Doc::page`): a left, centre and right part of
each, where `{page}` and `{pages}` stand for the page's number and how
many there are (`Page::fill`). Until it's set, pages are numbered at the
foot (so the window starts with `{page}` in the footer's centre); set,
exactly what it says shows, nothing if it's all empty. Each line sits
half way into its margin, in 10-point gray. It is kept in the file's
`user.olo.page` attribute and through a reload, and Word, OpenDocument
and RTF keep it (`olo_richtext::Document::header`/`footer`): Word's
header and footer parts (one paragraph, the parts a centre and a right
tab apart, `PAGE` and `NUMPAGES` fields), OpenDocument's master page
(`text:page-number`, `text:page-count`; its page now with inch margins
like the others) and RTF's `\header`/`\footer` with `PAGE`/`NUMPAGES`
fields; read back from any of them (complex Word fields too), and
checked in LibreOffice. Word's and OpenDocument's files also say tabs
past a paragraph's own stops go every half inch (`w:defaultTabStop`,
`style:tab-stop-distance`), as StyledEdit and RTF have them.

The tool bars (Document ▸ Show Toolbars; each window keeps its own
through a reload, new ones take the last choice) are the kit's
`ToolBar`, cached: along the top New, Open, Save, Print | Undo, Redo |
Cut, Copy, Paste | Find, Check Spelling | Bold, Italic, Underline,
Strikeout | the font, size and colour pickers | the four alignments |
bulleted and numbered lists | indent less and more, in two rows when the
window is narrower than one (the default window fits two); down the left
side Insert Picture, Insert Table, Header & Footer | Page View, Show
Ruler, Wrap Lines. Each tool is the menu command of the same name
(`tool_bars`); the pickers' menus are Font ▸ Size and Color and the
families of the Font menu (`size_menu`, `color_menu`, `families`, shared
with the menus), made from the document as they open.

Spelling is checked as you type (Edit ▸ Check Spelling While Typing,
on); right-clicking a misspelled word offers its suggestions, Ignore
Spelling and Learn Spelling above the Edit commands (`DocWin::spell`).

Other formats (`convert.rs`, on a worker, from a copy of the text, runs
and paragraphs): File ▸ Import… reads a Markdown, Word or
OpenDocument document (Markdown with pulldown-cmark) into a new, untitled one (headings, lists, quotes, code
blocks, bold, italic, struck-out and code text, links with their target
in `Style::link`; a table's rows as lines with tabs between cells, a
rule as a centred `* * *`), so saving it never writes over the source:
a `.md` opened with Open stays its plain text, as it was (people keep
notes in Markdown and edit them as text). File ▸ Export writes a copy as Word,
OpenDocument, Markdown (`to_markdown`: marks hug their text, Markdown's own
characters escaped, a list's items tight, code fenced; sizes, colours
and fonts have no Markdown) or a web page (`to_html`: every style, the
alignment, nested lists), leaving the document where it was. Word,
OpenDocument and RTF (`olo-richtext`, `from_rich`/`to_rich`): Import
reads `.docx`, `.odt` and `.rtf` too (and Open takes them there: they
aren't plain text; their tables come in as tables), and Export writes
any of them, each span's size,
font and colour said, in Olo's font, so they look in Word or LibreOffice
as they do here.

`fonts.rs` owns a font database on a worker thread: it lists families for
the Font menu (in submenus titled like a dictionary's guide words) and
loads a face the first time a style names it, registering it as
`se:Family:R|B|I|BI` with `style::add_document_font`; until then the UI
font stands in. Find and Replace are Haiku's windows (case, wrap-around,
backwards, replace all), shared by every document; ⌘G finds again, ⌘H
finds the selection, ⌘T replaces the same. `save_state` keeps every
window, with unsaved text and styles, its selection and where it was
scrolled to (`Editor::view`), through a reload.

## Activity Monitor (apps/activity)

Everything the machine is doing, in sections down a `Sidebar` (each row
with its figure now): Overview (cards of CPU, memory, network, disks,
GPU and the battery or the hottest sensor, each a way into its section,
and the busiest processes), Processes, Supervisor (what the supervisor
does, below: on or off, each finding as a card with its evidence and
Inspect, Stop Process and Ignore This Process, what it lowered, what it
did lately), CPU (by kind of time, each core
with its frequency, each cluster's frequencies and governor, load),
Memory (with zram's ratio, pressure stall information and every
/proc/meminfo counter), Disks (each disk's traffic, how busy it is, its
requests and their latency; volumes and how full they are; I/O
pressure), Network (each interface's traffic and addresses, the
programs using it most, every connection and its program), GPU (busy, power, memory, and each program's
GPU memory from DRM fdinfo), AI (brush's command model: suggestions
offered and taken, latency, how it learns; HACKING, "brush's
suggestions"), Sensors (the battery's health, cycles and
capacity; every temperature, power and fan sensor), System (the
machine, pressure, and the kernel's tunables, which can be set) and
Graphs (Haiku's ActivityMonitor: graphs of any sources, each scaled to
its own range, from the context menu). ⌘1 to ⌘9 are the sections
(`app_keys`: `activity.overview` and on). Click any graph to look back:
the Overview's last card says what was busiest at that moment (top CPU
and disk processes are kept for each sample, half an hour of them).

It is on the GPUI kit (olo-ui). `main.rs` has the app, an `Activity`
entity: the sampler's messages, the menus, the section shown, the Tune
dialog. Its window is cached `Part`s, each drawn from the app as it
changes (a sample, a choice) and replayed otherwise: the menu bar, the
`Sidebar`, the process list, the section's cards; and over the cards
a `GraphLayer` (GPUI draws again every view above one that changed:
this way a graph moving a pixel redraws the graphs and the window's few
boxes, and a caret blinking in the search the row it's in, not the
list). `sections.rs` draws the cards from the kit's dashboard (`Card`s
on the gutter in a `ScrollView`, `legend`s, `Facts` in one or two
columns, `meter`s, small lists of processes drawn as facts are; cards
side by side flow in as many columns as fit, given the room left of
the scroll bar), its graphs put on the layer (`Graph::on_layer`); the
Graphs section's cards share the window's height evenly between their
graphs. `processes.rs` is a `ColumnListView` whose rows (`Rows`: each
row's place, process and name) are made only when the processes, the
sort, the query, Tree or what's folded change; its cells read the
processes from the app as they're drawn, and a tree's triangles are
drawn by the Name cell and taken by `on_press` where they were drawn.
The filters' menus are made again only for a new list of processes.
Each inspector is an entity in a window of its own (as a tab of the
app's window where the session groups them), following the app's
samples (`observe`); its Process menu acts on its process. Settings
are written whole (beside, then renamed), on a worker as they change
and at once at Quit and before an upgrade replaces the program.

Against the egui Activity Monitor (release, 2× on a headless screen,
each section 20 s, the UI thread alone; the sampler thread is the same
code): Overview 1.8% of a core (egui 2.0), CPU 3.9 (5.2), Memory 2.7
(3.5), Disks 2.6 (3.4), Network 2.8 (3.9), Sensors 3.4 (4.7), System
3.2 (3.5), Graphs 2.2 (2.9), Processes 0.7 (0.4: its once-a-second
frame is 6 ms to egui's 3.7); the same frames a second (a graph's
pixel at a time: 11 to 23); 93–96 MB resident (119–123).

`sample.rs` reads /proc and /sys on a worker: `probe` finds the sources
once (cores, frequencies, disks of a gigabyte and up, interfaces, the
GPU via Asahi's `agx_stats` or amdgpu's `gpu_busy_percent`, every hwmon
sensor named for people); each sample has a value per source (NaN until
a rate can be known) and a `System` of what isn't a series (policies,
zram, pressure, disks' requests, interfaces, battery, GPU, tunables,
volumes every 5 s, which skips network and FUSE file systems because
`statvfs` there can hang). Processes are read every sample (their
name, command and owner kept by pid and start time); connections and
GPU clients only while their sections show (`Control`). The network
comes from netlink (`netdiag.rs`, as `ss` and `ip -s link` read it): a
`sock_diag` dump of every TCP and UDP socket, v4 and v6, with TCP's
`tcp_info` (bytes acknowledged and received, at 120 and 128), and
`RTM_GETLINK`'s `IFLA_STATS64` for each interface's counters (with
/proc/net as the fallback). Each process's Sent and Received (columns
in the list, and the Network section's "Programs Using the Network")
are its TCP sockets' bytes since the last sample, a socket new since
counting in full; whose socket is whose comes from the processes'
`/proc/PID/fd` (`socket:[inode]`), read again only when a socket no one
is known to have moves bytes, or every 5 s while the connections list
shows. A process whose files can't be seen (another user's, without
full access) has none. Settings saved before those columns came in at
8 have their sort column moved along (`Settings::migrate`).

A process's inspector (double-click, ⌘I, Inspect) reads `details.rs` on
a worker at each sample: General (program, command, folder, parent,
start, CPU time, scheduling, cores, context switches, I/O, control group,
namespaces it doesn't share, priority to change), Threads (CPU each, by
two readings), Open Files (sockets named as connections; double-click a
file for Tracker), Memory (smaps_rollup and mappings summed by file),
Environment, Resources (the cores it may run on, how the kernel
treats it when memory runs out, its limits: raise the soft limit to the
hard or lift both) and Supervisor (its findings, how sure and why,
whether it was lowered and why). Send Signal offers every common signal; Quit is
SIGTERM, Force Quit asks first. `actions.rs` does all of it (nice and
affinity for every thread; tunables only those in `TUNABLES`, governors
only those the CPU offers).

Processes chooses several the usual way (⇧-click a range, ⌘-click one
more, ⌘A all shown, ⇧ with the arrows): `Activity::selected` holds their
pids, following them as rows move, each leaving as its row leaves the
list (quit, filtered out), so an action never reaches a process not
shown chosen; in Tree a collapsed row stands for everything under it
(`chosen_pids`). The row menu, right-clicked on a chosen row, acts on
all of them ("Quit 3 Processes", Force Quit with one alert naming them:
"Force 3 processes to quit (sleep ×3)?", Send Signal, Priority checked
where all agree, Show Programs in Tracker each program once, Copy
Process IDs and Command Lines one a line), Inspect on the first; on a
row not chosen it chooses that one alone. The buttons under the list
and the Process menu act on the same set. Commands carry the pids
("quit:12,40,41", `pid_list`), so an agent's menu choice does the same;
refusals are said once (`act_all`), those wanting full access asked about
once.

Full access: `helper.rs` runs Activity Monitor's own program as root
(`olo-activity --helper`, through pkexec, which asks for the password
with Olo's polkit agent), and the sampler runs there, streaming samples,
details and actions' results back as JSON lines (NaN as null); the local
sampler pauses meanwhile. It asks at start (File ▸ Ask for Full Access
at Start, on by default), or when an action needs it (an alert offers
it, and does the action once it comes). It is only ever asked for in
the person's own session (`OLO_REAL_SESSION`, not on an agent's screen):
elsewhere polkit's dialog would come up on the person's screen.
`OLO_ACTIVITY_HELPER=direct` runs the helper as this user, without
pkexec (tests: the whole protocol, nothing elevated); `=off` never.
**In tests, only ever signal, renice or limit a process you started,
found by its exact ID (`pid:N` in the search).**

`olo-activity --section NAME` opens at a section (`supervisor`,
`cpu`, …); `olo-activity --inspect PID` opens only that process's
inspector, at its Supervisor tab (the supervisor's alert and Deskbar's
gauge menu use it; with a copy running, both are handed to it).

## Olo Doctor (apps/doctor)

Looks the computer over for what gets in Olo's way and fixes it with a
button: each finding's own, or Fix All (one password for everything that
needs root). `checks.rs` finds; each `Finding` carries its `Fix` as
`Step`s (`fix.rs`: write a file, set a sysfs knob, remove a file, start,
stop, enable or disable a system unit through `olo_system::systemd`,
signal a process), all done in process. From a terminal or an agent:
`olo-doctor --report` (the findings as JSON), `olo-doctor --fix` (those
with a fix, by id), `olo-doctor --fix all|ID…` (what the buttons do: root
steps through the helper and pkexec, or, under sudo, run there with the
scan made for the sudo user). What it looks for:

- **Input remappers** (keyd, xremap, Input Remapper, evremap, kmonad,
  Interception Tools): they grab the keyboards, and any mouse whose motion
  node has keys (a Logitech G309), and type through a virtual device, so
  Olo's key mapping and its per-mouse settings never reach them. Fixed by
  stopping and disabling the unit (or signalling a copy started by hand);
  one installed by hand under /usr/local is removed too (its /etc
  configuration is left).
- **Another desktop's daemons** in this session (GNOME Shell, Mutter,
  KWin, gsd-*): quit. Never another login's: a desktop shell of this user
  in another `session-N.scope` means that desktop is logged in there, and
  everything of it is its own.
- **GNOME's services Olo has its own in place of** (`GNOME_SERVICES`:
  dconf-service, GNOME's and GTK's portal backends, gvfs's daemons and
  volume monitors, GNOME Online Accounts, and GNOME Keyring while Olo's
  keyring answers), running in this session or as the user's services
  (not when GNOME is logged in elsewhere, nor an agent screen's:
  `OLO_SCREEN` in its environment, `on_a_screen`, its own session's,
  which starts its services again; the same for another desktop's
  daemons and old versions): advice, quit by the fix (what
  asks for dconf, a portal or the keyring reaches Olo's). None, and
  nothing of another desktop either: "Nothing of GNOME's runs".
- **USB input devices allowed to sleep** (an interface of class 03 with
  an input node under it, a receiver's paired devices a level down;
  `power/control` not `on`), and **Olo's udev rules** (71-olo-input,
  70-olo-displays; the Doctor carries them with `include_str!`) missing or
  out of date: installed and applied to what is plugged in now.
- **rtkit** not running (the supervisor's priority tiers need it).
- **Bluetooth not listening for the AirPods** (only with AirPods
  paired): BlueZ started without its experimental interfaces, where its
  advertisement monitor is. Fixed by a drop-in
  (`/etc/systemd/system/bluetooth.service.d/olo.conf`, `--experimental`)
  and Bluetooth restarted.
- **A USB 2 device in a Thunderbolt 5 port may go unseen** (advice, where
  Intel's Barlow Ridge USB controller, 8086:5782, is present): Ben's
  YubiKey in such a port wasn't seen at all (2026-11-04; the kernel heard
  nothing) while USB 3 drives were and the key worked in another port.
  It says to use another port meanwhile, and Keep It Awake for Now writes
  `on` to the controller's and its root hubs' `power/control` until the
  next restart, so plugging the device in again tells whether the
  sleeping controller misses the plug; once awake, it's fine.
- **Voice processing's sound not realtime**: a voice service's PipeWire
  thread that rtkit refused (its processes-per-user limit reached).
  Fixed by an rtkit drop-in (`/etc/systemd/system/rtkit-daemon.service.d/olo.conf`:
  64 processes, 256 threads a user), systemd reloaded, rtkit restarted;
  then Doctor itself, as the person (rtkit gives only to a thread's own
  user), asks it again for those threads before it looks
  (`checks::after_fix`, a fix's after-step, off the UI thread). Left to
  the supervisor's retry a minute on, the look just after the fix still
  found them refused and the finding came back after "Fixed.".
- **Frames kept off the fast cores**: `kernel.sched_util_clamp_min`
  below 1024 caps the `uclamp.min` app_server's and the windows' threads
  ask for (rule 9): set to 1024, now and at boot (/etc/sysctl.d/
  80-olo-uclamp.conf); a kernel without `CONFIG_UCLAMP_TASK` is advice.
- **Olo programs on an old version** (their `exe` is a replaced file in
  ~/.local/bin; never the session, app_server or the proxies): quit.
- **The login screen's entry** (/usr/share/wayland-sessions, or
  /usr/local/share's on an atomic system) missing or pointing at
  nothing.
- **Olo's own login screen** (`own_login_screen`, where Olo's build has
  olo-login and olo-greeter; not on an atomic Fedora, whose /etc and
  /usr/local Setup doesn't change; Ben, 2026-11-07: "the login screen
  should come up in olo doctor as something it can do for me"):
  another display manager's (`display-manager.service`'s link,
  `login_screen::display_manager`) is advice, "The login screen is
  GNOME's (GDM), not Olo's own", and Use Olo's Login Screen runs what
  Olo Setup's login-screen stage runs (`login_screen::steps`, the one
  list both use: programs copied to /usr/local/libexec/olo, data to
  /usr/local/share/olo, PAM, `pam_olo_keyring.so`, sysusers and
  tmpfiles, its user and folders, SELinux's labels, its unit, the other
  disabled and olo-login enabled), from the next start. Olo's already:
  fine, with Go Back to GDM (or SDDM) beside it as its undo
  (`login:own-undo`), unless a copy in libexec is older than the build
  in ~/.local/bin (an upgrade replaces those, not the copies:
  `login_screen::stale`), which is advice, Update the Login Screen
  (`login:own-older`).
- **Olo's Wi-Fi healer** (below) missing, out of date or stopped, on a
  machine whose Wi-Fi chip needs it (brcmfmac): installed as a root-owned
  copy, with its unit and NetworkManager's setting for the chip.
- **A second graphics card kept awake** (NVIDIA's, AMD's, not the one
  the computer started on; `power/control` "on", as it stays with no
  driver: nouveau failing on a new card): Let It Sleep installs
  `data/udev/72-olo-idle-gpu.rules` (runtime power management for such
  cards, at every start) and sets it now. On the ThinkPad P1 Gen 9 the
  RTX PRO 2000 went from D0 to D3cold.
- **A graphics card that stopped answering** (`gpu_stuck`: its
  `power/runtime_status` "error", the kernel's word for power
  management that failed, its driver not answering until it starts
  again: NVIDIA's after memory ran out on 2026-11-07, NVML saying "an
  internal driver error occurred"). NVIDIA's is started again without
  restarting the computer (Ben: "i don't want you to restart the
  machine just fix the card/driver or whatever"), Start It Again: the
  person's programs with its nodes open quit (`/dev/nvidia*`, its
  `/dev/dri` card and render node; named, each program once: Chrome
  writes its whole command line into its first word, so a process is
  named by its executable then), then one root step,
  `Step::RestartGpu`: the services that keep it open stopped
  (nvidia-powerd, nvidia-persistenced, Olo's own olo-gpu-follow, which
  holds NVML), its modules unloaded in order (nvidia_drm,
  nvidia_modeset, nvidia_uvm, nvidia_peermem, nvidia: `delete_module`
  with `O_NONBLOCK`, refused at once while anything has them open,
  asked again for 8 s, then the error names what has the nodes open),
  the card reset (`reset`: FLR, else its bus), taken off the bus and
  found again (`remove`, its bridge's `rescan`: a fresh device, its
  power management new), udev loading its driver by the card's PCI
  alias (nvidia_drm's) as at start, and the services started. Failing
  partway it leaves nothing off: the card's `uevent` told "add" (the
  driver loaded again) and the services started. Not while the
  session's display server has it open (a display on its ports): then
  what to do. Another maker's card stuck: said, a restart its fix.
- **A graphics card with no working driver** (NVIDIA's, no driver
  bound; nouveau loaded and gone says it tried): NVIDIA's driver
  (`akmod-nvidia`, built for each kernel) installed from the system's
  repositories when one has it (Fedora's NVIDIA repository, RPM
  Fusion's), else what to turn on; with Secure Boot, its key to enrol.
- **Intel graphics that can't decode H.264 and HEVC** (`intel_video`:
  an Intel GPU on Fedora, whose iHD leaves them out, and no full driver
  in `/usr/lib64/dri-nonfree` or `dri-freeworld`): RPM Fusion's
  `intel-media-driver` installed, its free and nonfree repositories
  added first where PackageKit doesn't find it (`Step::InstallRepository`:
  the release package downloaded from mirrors.rpmfusion.org, installed
  as a package file, the lists read again); on an atomic Fedora, what
  to do by hand. Fine once it's there ("Intel graphics decodes every
  video"). Olo's player decodes all four codecs there then
  (`olo_vaapi::decode`).
- **Built-in sound with no driver** (Intel's SOF bound, no sound card
  made of it: a new laptop's SoundWire codecs, named from
  `/sys/bus/soundwire/devices`): a newer kernel and firmware installed
  when PackageKit has updates for them (`DRIVER_PACKAGES`: the kernel,
  linux-firmware, alsa-sof, Cirrus and Intel audio firmware, GPU and
  Wi-Fi firmware), else said plainly that there isn't one yet.
- **Temporary files in memory** (`temp_files`, Ben 2026-11-05): /tmp a
  tmpfs (`olo_system::env::in_memory`, statfs) and the session's
  `TMPDIR` (systemd's user manager's, `env::manager_environment`, else
  Doctor's own) unset or in memory too. The fix (not root) writes
  `~/.config/environment.d/60-olo-tmpdir.conf` (`TMPDIR`, `TMP`, `TEMP` =
  /var/tmp; removing it undoes it), which the user manager reads at each
  login, and sets them in the running manager and D-Bus activation
  (`Step::SetSessionEnvironment`) for what starts from now on;
  olo-session takes them from that file as it starts, before any thread
  (`take_tmpdir`), so Deskbar's apps and Terminal's shells have them.
  Programs already running keep theirs until they start again.

Drivers and firmware come through PackageKit (`olo_system::packages`,
its D-Bus API: resolve, refresh, install, update, install a package
file; each a transaction waited on off the UI thread, its progress
followed): `Step::InstallPackages`, `Step::UpdatePackages` and
`Step::InstallRepository`, run as root by the helper, telling how they go (`Step::run_with`,
`FromHelper::Progress`) under the finding in the window ("Installing
akmod-nvidia: Downloading 42%") and on the command line. `Roots`'
`packages` and `updates` answer for the system's packages (tests: made
up).

Root is reached as Activity Monitor's is: the program run again through
pkexec (`olo-doctor --helper`, JSON lines of `ToHelper::Run { id, steps }`
and `FromHelper::Done`), asked for only in the person's own session;
`OLO_DOCTOR_HELPER=direct|off` as `OLO_ACTIVITY_HELPER`. After a fix the
report is taken again; what was fixed and is gone is listed under Fixed.
Scans run on a thread; nothing is polled: Check Again (⌘R), a fix, or
the app opened again looks again.

`olo-doctor --report` prints the report as JSON (agents, scripts).
`olo-doctor --watch` is one of the real session's services
(`REAL_SHELL`, with no window: it is never a running app in Deskbar):
after 20 s it looks, and every 15 minutes after (Ben: "run periodically
like on startup and then maybe every 15 mins"; a look is a few hundred
milliseconds of a background thread), and a problem not told of before
(remembered in `~/.local/state/olo/doctor.json`; one fixed and back is
told again) is told in a notification whose click opens Olo Doctor;
then it follows the Wi-Fi healer's report by inotify and tells of each
reset and what came of it (`--notify` is the look alone). Every look
(the watcher's, the window's, the command line's: `checks::look`) writes
what it found that gets in the way or has a fix
(`olo_protocol::doctor::DoctorFound`, `$XDG_RUNTIME_DIR/olo-doctor-found.json`);
Deskbar follows it by inotify and shows Olo Doctor's bag in the tray
while there is something, a click opening it. Tests build a
machine in a folder (`Roots { fs, bin, uid, scope, units }`) and check
the findings and their steps exactly.

Any finding can be ignored (Ben, 2026-11-07: "each item can be ignored
which puts it into an ignored section and also has a button for each
ignored item to still fix it if the user changes their mind but it
won't alert about ignored items or draw attention to them"): its
Ignore button, before its fix, keeps its id in
`~/.config/olo/doctor-ignored.json` (`ignored.rs`: `doctor` and
`suggestions`, by id), and every look sets those aside
(`checks::look`, `suggest::look`: `Report::set_aside` moves them from
`findings` to `ignored`), so the count, Fix All (Apply All), the tray's
bag, the watcher's notifications and `--fix all` leave them out. The
window lists them last, under Ignored: each title dim, with Stop
Ignoring and its own fix button, which fixes it as ever. From a
terminal or an agent: `olo-doctor --ignore ID…` and `--unignore ID…`
(`olo-suggestions` the same; with no ids, the ignored listed), and
`--report`'s `ignored`; `--fix` lists them as "ignored" and fixes one
only when it's named. A process list says each program once with its
processes ("GTK's portal backend (xdg-desktop-portal-gtk, processes
1625065 and 1625111)", `by_program`), and the title counts programs:
D-Bus had started GTK's portal backend six times, said six times over.

## Olo Suggestions (apps/doctor, `olo-suggestions`)

What Olo suggests for the best of it, none of it needed (Olo works the
same without): each with what it does and why, a button that applies it,
Apply All (⇧⌘A), and, once applied, a button that undoes it. It is Olo
Doctor's body (`apps/doctor` is a library with two programs: `app.rs`'s
window and command line by `Kind`), so a suggestion is a `Finding` whose
`Fix` is applied by the same steps and root helper, what's applied is a
`Fine` with an `Undo` beside it (`Report::undo`, found by
`Report::action` as a fix is), and `olo-suggestions --report` and
`--apply [all | ID…]` do from a terminal or an agent what the window
does (`all` never undoes). `suggest.rs` has them; `OLO_SUGGESTIONS_DEMO=
applied` shows them all applied, made up, to look at headless.

- **brush as the login shell**: brush (github.com/reubeno/brush), the
  bash-compatible shell in Rust, is built with Olo (`apps/brush`,
  `olo-brush`: `brush_shell::entry::run`, installed and upgraded with
  Olo's programs). Applied as root: Olo's build copied to
  /usr/local/bin/brush, listed in /etc/shells (`Step::AddShell`), tried
  (`Step::TryShell`: it runs `-c 'exit 0'`), and made the login shell
  through AccountsService (`Step::SetLoginShell`,
  `olo_system::accounts::set_shell`); the shell it was is kept first
  (`~/.config/olo/suggestions.json`) for the undo, else bash. A copy
  older than Olo's build is suggested again (Update brush). Terminal
  starts the account's shell as it is now (`pty::shell`: the password
  database first, `$SHELL` from login after), so its next window is
  brush at once.

- **olo-brush's process goes where the shell is**: brush keeps its
  working folder for what it runs, not as its own process's, so its
  `/proc/PID/cwd` stayed where it started, and Terminal (which reads a
  tab's folder there: its repository, worktree and PR, where a new tab
  opens) saw a tab `cd`'d into a worktree as still in its first folder.
  olo-brush's `prompt` hook (vendor/brush-interactive's, called as each
  prompt shows) moves the process there, as other shells move on `cd`.
- **Programs worth having** (`TOOLS`, `tool_list`): just, uv, mise and
  worktrunk (`wt`), each suggested while it's on no folder of PATH nor
  in ~/.local/bin or ~/.cargo/bin (where installers put them, which a
  session's PATH may not have), else said to be installed, and where.
  just and uv come from the system's repositories (`Step::InstallPackages`
  as root, PackageKit); mise and worktrunk, which Fedora doesn't carry,
  from their makers' latest GitHub release (`Step::InstallRelease`, as
  the person, to ~/.local/bin): `releases.rs` asks GitHub's API for the
  latest tag, downloads the asset (`{tag}` filled in: mise's bare
  `mise-{tag}-linux-x64`, worktrunk's `…-unknown-linux-musl.tar.xz`)
  and the SHA-256 its maker published (a SHASUMS256.txt, or the asset's
  own `.sha256`), refuses it if they differ, takes the program out of
  the archive (tar with liblzma or flate2) and puts it in place whole
  (written beside it as `.partial`, 0755, then renamed). Nothing is
  shelled out to; the download's share shows under the suggestion as it
  goes (`Msg::Progress`: a fix run as the person tells how it's going as
  the root helper's does). Olo doesn't take them away again. `cargo test
  -p olo-doctor installs_from_github -- --ignored` installs both, for
  real, into a scratch folder.
- **worktrunk's worktrees where Terminal makes them** (with `wt`
  there): its `worktree-path` in ~/.config/worktrunk/config.toml
  (`$XDG_CONFIG_HOME`'s if set) set to Terminal's place
  (`settings.terminal.worktrees`, asked of app_server) as worktrunk's
  template, `<place>/{{ repo }}/{{ branch | sanitize }}`: the folder
  Terminal's New Worktree… makes for any name it allows (no `/`, so
  `sanitize` changes nothing). Written with toml_edit, the rest of the
  file (comments too) as it was; the value it had is kept in
  suggestions.json for the undo (none: the key taken out again).

Each new suggestion is told once, ever, in a notification from Olo
Suggestions (`notify.rs`, run by the session's `olo-doctor --watch` at
each of its looks, after Olo Doctor's): "Olo has N suggestions" with
their titles, a click (or Show Suggestions) opening its window. What's
told is remembered by id and title (`Told::suggested` in
~/.local/state/olo/doctor.json, never forgotten, so one not taken isn't
told again, and Update brush after Use brush is), and one with nothing
to apply yet (brush before Olo's build of it) waits until it has. A look
costs under 10 ms (`olo-suggestions --apply` lists them: the whole
program, 0.00 s), four times an hour.

`--apply` with an id that isn't there (mistyped, or an option it
doesn't take) does nothing at all and exits 1, rather than applying the
ones it knows.
- **brush's suggestions from Olo's model (olo-complete)**: with
  Preferences ▸ Terminal ▸ Command Suggestions on
  (`~/.config/olo/complete.json` `enabled`, read by brush and the daemon
  with or without Olo's session), the grey hint after what's typed in
  brush is the model's.
  - `crates/complete` (`olo-complete`), the model and its arithmetic:
    `history` reads `.bash_history`/`.brush_history` (with `#<secs>`
    times) and Olo's record (`~/.local/state/olo/complete/
    history.jsonl`: each command with its folder, time and shell);
    `index` keeps per command its counts, folders, hours, weekdays,
    exit statuses, what follows what (one and two back) and each shell's
    last commands, offers the candidates that start with the line
    (at most 32) and gives each 16 features (`FEATURE_NAMES`). `kernels`
    is the model: 16 → 16 (ReLU) → a score, 289 numbers, a candidate's
    softmax over the line's candidates; scalar reference, NEON, AVX2+FMA
    (SSE2 without), each tested against the reference. (A GPU kernel was
    tried and dropped: 258 µs a suggestion through blade against 1 µs in
    NEON.) `Weights::baseline` is written by hand (the features weighed
    as they're meant). `train` replays the history as typing (examples
    at a few points in each command, from what was known before it) and
    fine-tunes with Adam pulled toward its start: the newest tenth of
    what it learns from checks each pass and the best pass is kept (the
    start among them), and the newest tenth of all, held out, judges the
    result, kept only if it ranks them as well (MRR; top-1 reported).
  - `apps/complete` (`olo-complete`, the daemon): one a person, on
    `$XDG_RUNTIME_DIR/olo-complete.sock` (`OLO_COMPLETE_SOCKET` instead,
    for tests: a test session shares the runtime folder with the
    person's), JSON lines, one request a connection (`protocol::Request`:
    suggest, ran, ended, accepted, continues, stats, learn, reload). `learn` is the
    first fine-tune, from the hand-written weights over everything; a
    minute after the first new command a quick one from the current
    weights over the newest, on its own thread (the model locked only to
    copy the examples and to put the weights in). Asleep otherwise (a
    condvar, and poll on the socket and an inotify watch of its folder:
    it exits when its socket is removed, as at logout, and when `reload`
    finds it off). `stats` is its account: suggestions offered and
    taken, latency from the question read to the answer sent (p50, p95,
    worst of the last 512), what it knows, the last fine-tune's report.
  - brush: `vendor/brush-interactive` adds `hooks` (`set_hooks`: the
    hinter, and each line entered with the folder and last status).
    `apps/brush` asks the daemon for a hint with 8 ms to answer, else
    shows brush's own; tells it each line entered, how the last ended
    and whether the hint was taken (whole, `complete_hint`, or a word);
    starts it, once a minute at most, when nothing answers. Its binary
    holds `protocol::HINTER_MARK`, by which `protocol::asks_for_hints`
    tells a brush that asks from one built before it did.
  - Preferences ▸ Terminal (`apps/preferences/src/complete.rs`): the
    checkbox (`protocol::OPTION`), usable when the login shell
    (`olo_system::accounts::login_shell`) is Olo's brush. On a worker:
    turning it on saves the setting, starts the daemon and waits for its
    first fine-tune ("Learning from your history…", then how it did);
    off saves and sends `reload`, and the daemon goes. Another shell, or
    a brush without the mark, gets a note and Open Olo Suggestions….
    `complete::lint_states` are linted (`ui_lint_command_suggestions`;
    the pane is the tallest's size).
  - Activity Monitor ▸ AI (`sections::ai`): `model_poller` asks `stats`
    once a second while the section shows, nothing otherwise; its
    latency is graphed (`ai.p50`, `ai.p95`). `OLO_ACTIVITY_DEMO_AI`
    shows `protocol::demo_stats` to look at headless.
  - The session and the screen (Ben, 2026-11-05): in Olo's Terminal a
    shell's session is its shell id, not its process (olo-termd sets
    `OLO_SHELL_ID`, and `OLO_SHELL_RESTORED` with the id of the shell a
    restored tab's stands in for; brush reads both as it starts, before
    it takes the environment into its variables, `protocol::shell_id`,
    and says `continues` once, `Index::continue_session`), so a tab's
    last ten commands (`SESSION_LAST`) carry on across a restart, from
    the record if the daemon started again too. olo-termd writes each
    shell's last 80 lines as text (`protocol::tail_path`, in the runtime
    folder, wrapped rows joined, `tail_text`) 300 ms after its output
    stops (`TAIL_AFTER`: nothing while idle), a restored tab's
    scrollback among them, and removes it as the shell or olo-termd
    ends. The daemon reads it when the file changed (`named`, cached by
    its time) for the commands it names (`output::commands`: what was
    run at a prompt, a line or backquoted text starting with a program
    the person runs: "Resume this session with:" and the command after
    it, `git push --set-upstream origin topic`, "run `cargo fix`"; the
    prompt being typed at left out), and the latest one that fits what's
    typed comes first, ahead of the model's
    (`output::tests::bens_restored_tab_suggests_the_resume`: Ben's
    restored tab, `cl` typed, his `claude … --resume 00000000-…`). Tried
    headless: a command printed by `printf`, `git l` typed in brush, the
    hint `og --oneline -3`. The installed `/usr/local/bin/brush` is Olo
    Suggestions' copy, brought up to date by its "Update brush".
  - Measured (M2 Max, release, 640 commands): a suggestion 1 µs in the
    daemon, 2 µs at p95 (26 µs round trip from a client); with a shell's
    tail (ThinkPad, 2026-11-05) 2 µs at the median, 3 at p95, as without
    one, and 40 µs once each time its tail changed; the first
    fine-tune 5 to 11 ms, a quick one about 2 ms; idle 0% of a core, 0
    wakeups a second; while Activity Monitor's AI shows, one a second
    in the daemon and 3 in Activity Monitor's poller (0.1% of a core).

## Olo Setup (apps/installer, tools/release.sh)

One program a person downloads from Olo's releases and runs on a stock
Fedora Workstation (Ben, 2026-11-06: "a full screen gui app that
\"olo-fies\" their fedora … windows 95 setup.exe vibes but windows 95
crossed with beos"). `olo-installer`, full screen, is Windows 95's
Setup in BeOS's look (`ui.rs`): the deep blue backdrop fading to black
with "Olo Setup" in big white bold italic serif and its shadow at the
top left (Liberation Serif, Times New Roman's metric twin, as Windows
95's), and on it one window at a time drawn as BeOS draws them (the
yellow tab with its title, the grey panel and its frame, from the
palette), each a page: a picture panel down the left (a sunken well,
the backdrop's gradient, the page's icon), the words on the right, `<
Back`, `Next >` and `Cancel` along the bottom. Welcome (what it does,
the password asked once, Windows 95's "it is strongly recommended";
why this computer can't, in red, if it can't: `plan::unsuitable`, not
Fedora 44 or newer, an atomic Fedora, root, under 6 GB free), Licence
Agreement (`licence.txt`: Olo's MIT terms, as Cargo.toml says, and the
others' work it carries, in a scroll box; Next only once "I accept"),
Choose What to Install (Olo Desktop always; Olo's login screen, its
boot splash and RPM Fusion, each with its words under it; what the
computer has already, chosen again and said so, "Installed already:
Setup brings it up to date": `Choices::installed_under`, Olo's login
screen as `display-manager.service`, Plymouth's theme `olo`, RPM
Fusion's repository file; the login screen runs copies of its own in
`/usr/local/libexec/olo`, which an update leaving it out kept old),
Installing
(a BeOS status bar, the stage over it and "n of m" at its right, what
it's doing under it, the stages and notes so far; a failure says what
and why in red, what to do, and Try Again), and Olo Is Installed (Log
Out, or Restart when the login screen or splash changed; and "Olo is a
work of love, dedicated to Sir Samwise, the bestest English Labrador
that ever walked the earth." in Ben's words).
About, from Welcome, says what Olo is and the dedication. None of Olo
is installed when it starts, so it carries what the kit draws with
(build.rs: Olo's fonts, its pages' icons at every size, the wordmark),
unpacks it into the runtime folder and points the kit there
(`OLO_DATA`).

The work (`plan.rs`) runs on a worker, each stage told to the window
(`Event`), each doing only what isn't done, so it can be stopped
(Cancel) and run again: checking the computer; what Olo needs from
Fedora (`PACKAGES`: what its programs link, `ldd` on every program,
22 packages, and the services and fonts a session uses) through
PackageKit; RPM Fusion's two repositories if chosen (Olo Doctor's
`InstallRepository`); Olo itself (`bundle.rs`: the newest GitHub
release's `olo-VERSION-ARCH.tar.zst` of `benmyles/olo`, or `--bundle
URL|FILE`, downloaded into `~/.cache/olo-installer` and kept, its
published SHA-256 checked, unpacked (zstd, tar; nothing outside `olo/`)
and put in place as tools/install-session.sh does: each program beside
its old self then renamed over it, the data copied over, `op`, `pbcopy`
and `pbpaste` linked, the version in `~/.local/share/olo/version`);
Olo on the login screen's menu (its session entry naming
`~/.local/bin/olo-session`, Olo's udev rules, the devices they're for
told to look again by their `uevent`); and if chosen, Olo's login
screen (tools/install-login.sh `--switch`'s work: its programs in
/usr/local/libexec/olo, Olo's data in /usr/local/share/olo, the PAM
services and `pam_olo_keyring.so`, its user and folders made by
`systemd-sysusers` and Setup itself, its unit, the display manager
it replaces disabled (GDM's) and olo-login enabled: Olo Doctor's
`login_screen::steps`, which its "Use Olo's Login Screen" runs too) and boot splash
(the theme drawn by `olo-app-server --plymouth-theme` as the person,
then put in /usr/share/plymouth/themes/olo with plymouthd.conf kept
beside as `.before-olo`, and the start-up images made again:
`Step::RebuildInitramfs`, dracut). Two steps run another program,
each the system's own and with no other way in: dracut, and
`systemd-sysusers` on the login screen's own file
(`Step::MakeSystemUsers`: systemd's sysusers service runs only after an
update, `ConditionNeedsUpdate=/etc`, so restarting it made no user);
the tmpfiles entry's folders are made by Setup itself (`Step::MakeDir`:
systemd refuses to restart its tmpfiles service by hand). What
needs root is Olo Doctor's privileged helper (`helper.rs`, the same
`Step`s, two added for Setup: `CopyTree` and `RebuildInitramfs`), this
program run again through pkexec (`--helper`), asked for once: the
password goes to the desktop's own polkit agent, GNOME's on a stock
Fedora; `OLO_SETUP_HELPER=direct` runs it as the person (tests: what
needs root then fails, safely). PackageKit's lookups now find what is
installed first and only look for the rest in this architecture
(`olo_system::packages::resolve`: asked for the newest alone, an update
waiting showed as not installed, and installing what was missing
became an update, which asked for a password). `--yes` goes through the
pages by itself, a page every four seconds (`--login-screen`,
`--boot-splash`, `--rpm-fusion` choose), for tests and agents; `--check`
says as JSON whether this computer can have Olo and opens nothing.

The release (`tools/release.sh [OUT]`, from a release build): every
`olo-*` program stripped (300 MB with line tables to 30 MB; a third of
that compressed), `pam_olo_keyring.so`, Olo's data as
install-session.sh copies it and what goes outside the home folder
(`system/`: the session entry, udev rules, PAM services, the login
screen's unit, user and folders), and `VERSION` (the date and the
commit) in `olo-VERSION-ARCH.tar.zst` (zstd -19), and the installer
stripped beside it, each with its `.sha256`. Publishing them (a GitHub
release) is a separate step.

`tools/dev/installer-check` tries it all in a virtual machine (KVM, a
virtio GPU, no window, driven through QEMU's monitor; `boot --gl
RENDER` gives it a GPU with 3D, virgl through one of this computer's
render nodes, as Olo needs one to draw: Fedora's GNOME draws in
software on the plain one, Olo can't and its login screen steps aside;
its screen is then read through a VNC socket of its own, as QEMU's
screendump has no picture of a 3D scanout, and here only through the
NVIDIA node's EGL): `install` puts a
fresh Fedora Workstation on a disk from Fedora's own network installer
(a kickstart choosing Workstation's package set; for the test alone,
GDM logs the tester in and GNOME's first-run tour is skipped), `boot
--serve DIR` starts it with the release served to it at
`http://10.0.2.2:8642/`, and `shot`, `type`, `key` and `click` look and
act. Everything is kept in /var/tmp/olo-installer-check.

## The Wi-Fi healer (apps/wifi-heal)

The Broadcom chips in Apple machines (brcmfmac; BCM4388 here) can get
stuck in their firmware: every join fails without associating
(wpa_supplicant's `ASSOC-REJECT status_code=16`, which brcmfmac reports
for any failed join), address changes are refused (`-52`, NetworkManager's
"failed to set MAC address"), and scans stick ("Reject scan trigger since
one is already pending"); NetworkManager then asks for the password again
and says it was wrong. It lasted 38 hours on 2026-10-31. Resetting the
device (the driver unbound and bound again, which reloads the firmware)
joins at once, measured; a run of failed joins can bring it on (it did,
in testing, after two resets and four refused joins). Joining early
after the firmware starts, and a regulatory change, were measured not to.

`olo-wifi-heal` is a root service (a root-owned copy in
/usr/local/libexec, never the person's own file, which anything running
as them could replace) that follows NetworkManager's device states and
wpa_supplicant's interface states as D-Bus signals, nothing polled.
When a join fails after two or more tries to associate with none getting
further, or a scan runs two minutes, it resets the chip and joins the
same network again. On this chip a wrong password fails the same way
(the firmware does the handshake), so the reset is the test: if the
join after it fails alike, the chip wasn't at fault, which is recorded
("its password is most likely wrong"), and that network isn't reset for
again until it has joined. One reset in three minutes, four an hour,
only for drivers known to recover so (`DRIVERS`). Each goes in the
journal and `/run/olo/wifi-heal.json` (`olo-wifi-heal --status`), which
`olo-doctor --watch` tells the person of. `OLO_WIFI_HEAL_DEBUG=1` logs
every event. Test it as root with a copy in /var/tmp and a profile with
a wrong password (the chip may stick after; the healer frees it).

`data/NetworkManager/80-olo-brcmfmac.conf` (installed with it) keeps the
chip's own address while scanning, so NetworkManager doesn't change it
every few minutes while idle; each network still gets its own address.
Olo's join (`olo_system::network::join`) asks for the security the access
point offers (`key_mgmt`): WPA3 alone is "sae" (asking for WPA2 was
refused before it began: "Access point does not support PSK"), Enhanced
Open "owe".

## Voice processing (crates/voice)

`olo_voice::Voice` processes one microphone, a 10 ms frame (480 samples
at 48 kHz) at a time: `render` takes what the speakers play, `capture` a
microphone frame and gives it back processed; `latency_samples` says what
it adds (30 ms with noise suppression, 10 ms with echo cancellation
alone, 0 with neither). `Settings` (mode, echo cancellation, noise level,
normalization) choose the chain; build a `Voice` off the real-time thread
(loading the model takes a moment). In order:

1. **Echo cancellation** (`echo.rs`): finds where the echo is (coherence
   over 0-500 ms, moved only when clearly better than where it is), then a
   partitioned-block frequency-domain Kalman filter, 120 ms of room, over
   the whole band, in two paths (Speex's: a foreground copy the output
   comes from, kept safe while both people talk). Then residual echo
   suppression: a Wiener gain per frequency from the filter's own
   uncertainty, applied to the noise model's input spectrum (so it never
   hears the other person's voice, which would make it turn yours down).
   No delay of its own. About 30 µs a frame.
2. **Noise suppression**: DeepFilterNet3 with two frames of look-ahead.
   Thresholds: noise-only frames go down to the limit, both decoders always
   run otherwise (the library's defaults chop clean speech). About 250 µs
   a frame. Its weight matrices are f16 (`deep_filter::tract::
   half_weights`: each matmul's constant f32 weights swapped for an f16
   copy before optimizing, so tract packs them as f16 and runs its
   f16-weights, f32-activations kernels; the sums and activations stay
   f32): half the bytes to fall out of the caches between frames. On x86
   with F16C a frame back to back went from 391 to 296 µs and one every
   10 ms, as live, from 773 to 536 µs, the voice trials' SDR the same to
   a tenth of a dB (translating the whole model to f16, activations too,
   had wrecked it). Where it doesn't optimize (a processor with no such
   kernels) it's f32; `OLO_VOICE_F32=1` asks for f32 to compare.
3. **Level normalization**: AGC2 (adaptive digital gain from a neural
   voice detector, then a limiter), allowed to turn loud talkers down as
   well as quiet ones up: speech lands within half a dB of -24 dBFS rms
   from anywhere between -40 and -1 dBFS peaks.

`Mode` (Calls, Recording) doesn't change the processing, only the
buffering around it. The FFTs are rustfft's (NEON, AVX, SSE at run
time), the model runs on tract's NEON/AVX kernels, and the canceller's
per-frequency math is laid out as separate real and imaginary arrays in
straight loops, which compile to vector code (check with objdump: `.4s`
instructions in `EchoCanceller::cancel`). A GPU doesn't fit: a frame is
well under a millisecond of work, shorter than a round trip to a GPU
queue, and every frame depends on the one before.

`cargo test --release -p olo-voice -- --nocapture --test-threads=1`
prints what each stage does on synthetic speech (CMU Flite voices,
`tests/data`), room noise, music and the other person's voice over a
synthetic echo path: noise and echo taken out, the voice's level kept
(frame by frame, so that a high-pass filter's harmless phase shift
doesn't count as damage), delays as stated, and the time a frame takes.
Don't judge a speech model with tones: DeepFilterNet takes a synthetic
buzz for noise.

## The voice service (apps/voice, `olo-voice --service`)

Every microphone through voice processing (`crates/voice`, above). The
service puts a source of its own, named after the microphone it listens
to ("AirPods (Voice Processing)", `Config::description`, renamed live
as the microphone changes: `pw_stream_update_properties`, which every
list of devices, PipeWire's and its PulseAudio server's, follows;
`olo_protocol::voice::NODE`, `olo_voice`), in front of the microphone
the person chose and makes it the default input, so anything that
records gets the processed voice, and the processed device and the raw
one are always told apart:

- **Which microphone**: whatever becomes the default input that isn't
  its own (Preferences, Deskbar, AirPods' choice, any program) is the
  one it listens to, and it is the default again. Device lists
  (`olo_system::audio::devices`, `watch_devices`) leave its source out
  and show that microphone as the default; `devices_raw` has both.
  Turned off, stopped or quitting, the microphone is the default itself
  again; if it crashes, WirePlumber falls back to the microphone.
- **What's behind the source** (`engine.rs`, PipeWire's thread): the
  microphone's capture and, for echo cancellation, the default output's
  monitor, only for an output the microphone can hear (not
  `Device::private`: Bluetooth, a headphones or headset port or form
  factor). Made inactive, switched on only while something records from
  the source: idle, nothing runs and the microphone isn't open. A new
  microphone or new settings rebuild only the captures, so a call keeps
  its device. Calls ask the graph for a 10 ms quantum and keep a frame
  ahead; recordings 2048 samples and four frames.
- **Settings** `voice.json` in the config folder
  (`olo_protocol::voice::Settings`: enabled, mode, echo cancellation,
  noise, normalize), followed by inotify. **State** `olo-voice.json` in
  the runtime folder: the microphone, recording or idle, echo cancelling
  now, the delay it adds, a frame's processing time, dropouts.
- **Command line**: `olo-voice --status [--json]`, `olo-voice --set
  KEY=VALUE…` (`enabled`, `echo`, `normalize`: on/off; `mode`:
  calls/recording; `noise`: off/moderate/strong/full). Deskbar's menu
  uses it too.
- **None as the output** (Preferences ▸ Sound, first in Play sound
  through; first in Deskbar's speaker menu): sound goes nowhere rather
  than to the only device there is (a network speaker in another room).
  It's a silent sink of Olo's (`olo_system::audio::NO_OUTPUT`, PipeWire's
  null sink, loaded over the PulseAudio protocol: `set_no_output`) made
  the default, never listed as a device (`Devices::no_output`).
  `olo_protocol::sound::SoundChoice` remembers it, and Deskbar makes it
  again at login (the sound server forgets it).
- **A Bluetooth device that plays nothing** (Ben's AirPods, 2026-11-04:
  an input, but no output): `Devices::unplayable` lists each Bluetooth
  card (`api.bluez5.address`) with no output of its own, why
  (`Silence`: its profile `off`; no music or call profile available, its
  Bluetooth audio not connected; or one chosen yet no output) and the
  profile that plays (an available `a2dp-sink*`, else
  `headset-head-unit*`). Preferences ▸ Sound names it under Output ("…
  is connected but plays no sound: its sound is turned off.") with Play
  Sound Through It (`set_card_profile`). The watcher follows cards too
  (`SubscriptionMask::CARD`), so it goes as the profile comes up.
- **UI**: Preferences ▸ Sound's Voice Processing box (and what it's
  doing in a line); Deskbar's speaker menu: "Clean Up Microphone Sound"
  and a Voice Processing submenu.
- **Try It…** (Preferences ▸ Sound ▸ Voice Processing,
  apps/preferences/src/voice_try.rs): six seconds recorded from the
  microphone the service listens to (`State::microphone`, not its own
  cleaned-up source) with `olo_system::recording::Recording` (the
  PulseAudio protocol's record stream, mono 48 kHz float, in memory
  only), then put through each setting worth comparing on a worker
  (`variants`: as set, each other noise level, the level the other way,
  the other mode; `olo_voice::trial`, after a short untimed one: a
  process's first chain sets up what the rest share, a 27 ms first
  frame against 0.4), each listed as it's done: the delay it adds, its
  time a 10 ms frame (median and worst), what noise suppression and
  levelling did (`Stage::change_db`) and what ran it
  (`accelerated_by`). Played with `recording::Player`, the original or
  the chosen setting's sound, switching at the same place (one stream,
  the take chosen by an atomic; 60 ms ahead at most, so a switch is
  heard at once), with a live spectrum of both (`olo_voice::spectrum`,
  under 40 µs a call). "Use This Setting" saves it. Echo cancellation
  isn't tried: a recording has no speakers' sound beside it. Tested in
  a `tools/dev/pw-sandbox` (`recording::tests`, whose "talk" sink the
  "mic" source hears: two tones played, switched halfway, recorded
  back). Measured: 292 µs a frame while playing (the whole window
  drawn each frame was 1.8 ms), 252 µs while recording, nothing idle.
- **Running it**: olo-session starts it in a real session (`REAL_SHELL`)
  and restarts it if it exits; an upgrade installs the new `olo-voice`
  and sends SIGURG, on which it starts again as the new version (the
  source comes back, the default stays); SIGTERM gives the microphone
  back. The supervisor never lowers it and keeps it in the audio tier
  (above). Olo Doctor checks its sound is realtime.
- **Testing**: in `tools/dev/pw-sandbox`, never the person's sound
  server: `OLO_VOICE_DIR=DIR` keeps its settings and state apart, and
  `pw-cat --playback --target talk speech.wav` talks into the made-up
  microphone while `pw-cat --record` records the default input. Measured
  there: the voice comes through with no dropouts; another voice on the
  speakers with its echo in the microphone goes from −24…−63 dBFS
  (processing off) to −89…−99 dBFS; idle 0% and 0 wakeups, recording
  5.8% of a core (388 µs a frame).

## AirPods (apps/airpods, olo_system::airpods, Preferences ▸ AirPods)

Olo talks to AirPods itself, in Rust, as Apple's devices do. Their
sound is BlueZ's and PipeWire's like any headphones' (AAC at music
quality; the headset profile for calls); everything else is Apple's
accessory protocol (AACP), written from the protocol's public notes
(LibrePods' docs, the apple-wireshark dissector), not from anyone's code:

- `olo_system::airpods::proto`: the messages, both ways. `04 00 04 00`, a
  16-bit opcode, its bytes: the hello (nothing is answered before it),
  host features (Conversation Awareness while sound plays, Adaptive),
  notifications, the broadcast keys; levels (left, right, case, or
  over-ear headphones' one), where each bud is, every setting (opcode 9:
  listening mode, the modes press and hold cycles through, Conversation
  Awareness, Personalized Volume, Adaptive's strength, ear detection,
  one-bud Noise Cancellation, volume swipe, case sounds, which bud's
  microphone, press speed and hold duration), Conversation Awareness's
  levels, who they are (model number, serial, firmware), renaming.
  Unit-tested against documented captures.
- `link`: the L2CAP channel (PSM 0x1001, a SEQPACKET socket through
  libc), opened once BlueZ has connected them.
- `adv`: their LE proximity broadcast (Apple's manufacturer data, kind
  7): model, levels, ears, connected or not, and the block sealed with
  their encryption key (levels to the percent); `resolves` checks their
  rotating private address against their identity key (tested on the
  Core Specification's sample).
- AirPods are known by BlueZ's Modalias (`bluetooth:v004Cp2024…`:
  Apple, product 0x2024, AirPods Pro 2 USB-C); `Model::of` says what
  each can do.

`olo-airpods --service` is a real session's service (`REAL_SHELL`). It
follows BlueZ by its signals (`bluetooth::watch`: connections, pairing,
names; a search's signal strengths ignored), opens the channel to each
pair connected (a thread each, asleep on it; tried again for a few
seconds while their profiles come up), and follows the sound server
(`audio::watch_devices`; a Bluetooth device's address, codec and
profile come with it, `audio::Bluetooth`, and cards and their profiles,
`audio::cards`). What it does, by Olo's settings (`airpods.json`,
`AirPodsSettings`, followed by inotify): pauses what plays (MPRIS,
`olo_system::media`) when a bud comes out, if the sound is going to
them, and plays it again when it's back (within ten minutes); turns
their output down while Conversation Awareness says the wearer speaks,
and back after; keeps the computer's microphones as the input (an input
of theirs that has just come and been made the default is undone), so
they stay at music quality, unless the person chose theirs. It keeps
their broadcast keys in `~/.local/state/olo/airpods-keys.json` (0600).

Their broadcast (`olo_system::bluetooth::watch_apple`: BlueZ's
advertisement monitor, passive, its devices' ManufacturerData followed
by signals) is resolved to a pair by its key, gives their levels while
they aren't connected here, and says whether they're nearby and
connected elsewhere (`AirPods::nearby`, `elsewhere`). With it: **connect
automatically** when a bud is put in an ear and they're connected to
nothing; **switch automatically** when something starts playing here
(`audio::watch_playing`) while they're in an ear on another device (the
phone takes them back itself when it plays). **Never a tug of war**:
once they stop being connected here (a phone took them, or the person
disconnected them), nothing is taken by itself until their broadcast has
said "connected to nothing" for 60 s (`may_take`, tested), and one try
every 15 s at most. BlueZ 5.87 has the monitor among its experimental
interfaces: without `bluetoothd --experimental` the state says so
(`AirPodsState::not_listening`), Preferences greys the two settings, and
Olo Doctor offers to turn it on.
It publishes `$XDG_RUNTIME_DIR/olo-airpods.json` (`AirPodsState`) on
every change, and answers `Request`s (JSON lines) on
`olo-airpods.sock`; after an upgrade (SIGURG from olo-session) it
starts again as the new binary. Idle it is asleep on BlueZ's signals,
the sound server's events and the channels: nothing is polled.

**Their own microphone** (`mic.rs`): while a pair's channel is open,
the service has a PipeWire source for it ("… Microphone",
`airpods_mic.ADDRESS`, made in process with pipewire-rs on a thread of
its own, `priority.session` 100 so it is never the input by itself). It
starts the AirPods' wideband stream (`proto::start_microphone`) only
while something records from it (the stream streaming) and stops it
1.5 s after recording stops, so idle it costs nothing, nor their
battery; Conversation Awareness is off meanwhile (speaking would duck
the sound), and their sound stays at music quality (A2DP AAC; checked
by ear on a lossless stereo track). Their frames (AAC-ELD, four of 480
samples a message at 64 kHz, though the config says 48) are decoded by
`olo-eld`; frames lost on the air (a gap in the frames' 24 kHz
timestamps, 180 a frame) are decoded empty, so the sound dies away
through the filter bank rather than stopping. `resample.rs` takes them
to 48 kHz (windowed sinc, Kaiser, 32 taps, 512 interpolated phases),
its ratio steered within ±0.5% to keep 60 ms waiting (the AirPods'
messages come every 31 ms on median and up to 53 ms apart, measured;
150 ms when voice processing is tuned for recordings, read as the source
starts streaming):
the two clocks' difference is followed, never by dropping or repeating
samples, and a source run dry fades out over 5 ms and back in. Each
cycle is given exactly what it asks for (`Buffer::requested`). Checked
on AirPods Pro 2: 12 s recorded, no gap, never dry, none lost, the
largest step between samples −50 dBFS (room noise); sound begins
320 ms after recording does (the AirPods' start, then the 60 ms).
`OLO_AIRPODS_CAPTURE=FILE` keeps the raw frames (to test `olo-eld`
against). `--microphone on|off` drives it by hand.

The command line asks it, for people and agents: `olo-airpods`
(`--status [--json]`), `--mode off|transparency|adaptive|anc`, `--set
SETTING VALUE`, `--rename NAME`, `--connect`, `--disconnect`,
`--quality music|calls`, each with `--address` for one pair of several.

Where they show: Preferences ▸ AirPods (`airpods.rs`, opened from
Bluetooth's "AirPods Settings…", Sound's and Deskbar's: name, levels
with a picture of each bud and the case, noise control, press and hold,
listening features, the microphone, and On This Computer); Bluetooth's
list (their model, their lowest level); Sound (under the output: model,
codec, music or call quality, noise control; in the input: where to
record from while they're connected); Deskbar's tray while they're
connected (`status::draw_airpods`, amber at 20%, red at 10%; its menu
has their levels, noise control, Conversation Awareness, Disconnect and
both panels), and the speaker's right-click menu (`sound_menu`, for any
device: every output and input, mute, voice processing (on or off, and
its settings in a submenu), and while AirPods play, their levels, noise
control, Conversation Awareness, Personalized Volume and music or call
quality). Deskbar follows the sound server's whole device
list (`audio::watch_devices`), the volume being the default output's. Their icons are `airpods`, `airpods-case`, `airpod-left`
and `airpod-right`, in every theme.

**Tests only look: never pair, connect or disconnect real AirPods.**
`OLO_FAKE_AIRPODS=1 olo-airpods --service` pretends a pair (AirPods Pro
2 USB-C, connected, taking every change); `OLO_AIRPODS_DIR=DIR` keeps
its state and socket in DIR rather than the runtime directory, so give
it to the service and to what reads it (Preferences, Deskbar: start the
session with it in the environment). `olo-fakesound` with
`FAKESOUND_AIRPODS=AA:BB:CC:DD:EE:FF` adds their output (AAC, music
quality, the default) and card, for Sound.

## The supervisor (apps/supervisor, Preferences ▸ Performance)

Keeps the desktop responsive; off by default. `olo-supervisor` is one of
the real session's services (`REAL_SHELL`): it waits, following
`performance.json` (`olo_protocol::performance::PerformanceSettings`),
until turned on in Preferences ▸ Performance, by Deskbar's gauge (a
click; its menu has the findings, Show in Activity Monitor and
Performance Settings…) or `olo-supervisor --on`. Then every two seconds
(about 6 ms on its own thread; each command line read once a process)
it looks at the person's own processes in /proc (`sample.rs`) and:

- **Tiers first** (`olo_protocol::performance::tier`, Ben, 2026-11-01):
  what keeps the desktop responsive runs above everything else, always,
  supervisor or not: app_server's main thread (every input event and
  frame) at nice −10, olo-session, Deskbar and olo-wlproxy at −5, each
  asking rtkit for itself as it starts (`olo_system::priority::raise_self`:
  `org.freedesktop.RealtimeKit1`'s `MakeThreadHighPriorityWithPID`, the
  one way an unprivileged process gets a negative nice, down to its
  `MinNiceLevel`, −15 here), and each frame's thread on a fast core
  (its `uclamp.min`, rule 9; no privilege needed). The supervisor
  re-asserts a tier for a
  program found above it (rtkit refused at its start, or a thread made
  since; `priority::raise_process`, every thread), on every pass: one
  lowered since (reniced) is raised again, one rtkit refused is asked
  again a minute later. app_server's main thread matters most: it reads
  the pad and Olo's touch filter judges palms on it (the palm model, as
  each frame comes), and it draws. It publishes what it raised
  (`SupervisorState::raised`; Activity Monitor's "Olo's own" row) and
  notes once a program it couldn't. Only then the focused app over
  background work, which is the lowering below.
- **The voice services** (olo-voice, olo-airpods: the audio tier,
  `AUDIO_NICE`, −10 as app_server's): raised as they start, their
  processing thread on a fast core (`AUDIO_UTIL_MIN`). PipeWire's thread
  for their sound (`data-loop`, `AUDIO_THREAD`) should be realtime:
  libpipewire asks rtkit as a stream starts. The supervisor looks: one
  that isn't is asked for again (`olo_system::priority::realtime`: a
  200 ms RLIMIT_RTTIME set first, which rtkit wants), and a minute after
  each refusal. rtkit gives realtime to 15 processes a user by default;
  a desktop's programs use that up, and Olo Doctor offers more.
- **Priorities** (`priority.rs`): background work, known by name (the
  settings' list: compilers, linkers, build tools, indexers) or a
  terminal's job busy for a minute and more, has every thread lowered:
  nice up to the setting's (never down: unprivileged, it couldn't come
  back), `SCHED_BATCH`, idle I/O priority. Never the active window's app
  and what it started (followed from app_server's `WindowsChanged`), and
  never Olo's own programs (by name, or beside the supervisor). Turned
  off, batch and the I/O priority are put back (a raised nice stays until
  its process ends); a new supervisor (an upgrade) takes over what the
  last one lowered from its published state, or puts it back if off.
- **Watchdog** (`detect.rs`), each from several signals, with a
  confidence and its evidence: stuck in a loop (busy at least `spin_cpu`
  of a core in every sixth of `spin_minutes`, waiting on almost nothing,
  almost no I/O, its memory unchanged); leaking (anonymous memory and
  swap, one sample a minute for three hours, rising by a least-squares
  fit at least `leak_fit` R² over `leak_minutes`, by `leak_rate_mb` a
  minute and `leak_growth_mb` in all, most of its steps up); too much
  memory (`memory_share` of RAM while less than `memory_available` is
  free, or tasks stall on memory (PSI) more than `memory_stall`, worse
  if it's growing or the system swaps); zombies (a parent leaving
  children unreaped `zombie_minutes`, urgent at `zombie_count`). Every
  number is in `Thresholds`, tuned in Preferences.
- **Urgent findings** (stuck for sure, memory that runs out, many
  zombies) ask once a process and kind in an alert (`olo.supervisor`,
  named "Performance"): Ignore This Process (into the settings'
  `ignored`, by name or command), Show in Activity Monitor (`--inspect`),
  Stop Process (TERM, then KILL after 3 s). Not asked again after an
  upgrade.

It publishes what it does in `$XDG_RUNTIME_DIR/olo-supervisor.json`
(`SupervisorState`: on, its pid, what it favours, findings, what it
lowered and why, the last 50 things it did, memory); a state whose
supervisor has gone reads as off. Activity Monitor's Supervisor section
shows it as cards (and turns it on or off), each process's inspector a
Supervisor tab; Deskbar's gauge (`status::draw_gauge`, shown when it is
installed) counts the findings, red when one is urgent. Agents read
`olo-supervisor --status [--json]` and drive the same controls.
**Test it with a private `XDG_RUNTIME_DIR` and
`OLO_SUPERVISOR_ONLY_UNDER=PID` (only that process and what it started
are looked at), never on the person's own processes; and give every
program it may start `OLO_ACTIVITY_HELPER=off`.**

## DriveSetup (apps/drive-setup, olo_system::disks, udisks-fake)

BeOS's DriveSetup with Haiku's disk map: the chosen disk drawn to scale
(`map.rs`: every stretch at least 48 px so small partitions keep their
labels, coloured by family: Linux, Windows and everywhere, encrypted,
swap; an unlocked volume shows what is inside under a purple band; free
space hatched) over a `ColumnListView` of every disk, its partitions
(logical ones under their container, unlocked contents under what holds
them) and its free space. The Disk and Partition menus and the context
menu act on the selection; double-click opens, mounts, unlocks or
creates, whichever fits.

`olo_system::disks` is the UDisks2 client (blocking, so call it from
a worker): `status()` reads `GetManagedObjects` into `Disk`s, `Volume`s
and `Job`s (all `Serialize`); each mounted volume's
`space` (`Space`: size, used, free, from statvfs at its first mount
point) comes with it; `watch(f)` calls `f` after each burst of
UDisks signals; the operations (mount, unlock, format with an optional
LUKS passphrase, create/delete/resize partitions, label, new table,
eject, SMART update, cancel a job) map UDisks errors to words with
`error_text`. `release(volume)` unmounts and locks what is in use before
a format or delete. `write_image` streams a file onto the fd from
`OpenForRestore`; `benchmark` reads spread samples and random 4 KiB
blocks from `OpenForBenchmark` (read-only). UDisks itself asks polkit.

The list's Free column shows a mounted volume's free space after a
small bar of how full it is (red past 90%), and the status line says
"N free of M"; while the window is open the app re-reads every 5 s,
since writing files sends no UDisks signal.

The app runs one step at a time (`DriveSetup::run`: a worker, reporting
progress; Stop sets its flag, or cancels the UDisks job), shows UDisks'
own job progress in the status line, and re-reads on every change.
Destructive steps are behind an alert or form naming the disk and
device; nothing destructive is offered for the disk or volumes holding
the running system (`Volume::system`, `Disk::system`). On the GPUI kit
(olo-ui): the forms (Format, Create, Resize, Rename, Unlock, New
Partition Table) are `Form` views in dialogs drawn as alerts with
fields (`AlertFrame`, `paragraph`, `button_row`, the alert's key
context: Return submits, also from a field, and Escape cancels), each
window resized to what it shows as fields come and go; the benchmark
is a window of its own; the map's segments after the first are their
own grid for the layout review. A periodic re-read that finds nothing
changed draws nothing. Agents: `--list` prints
`Status` as JSON; `--mount`, `--unmount` and `--eject DEVICE` do the safe
steps.

**Never test on real disks.** `OLO_UDISKS_BUS=ADDRESS` points the
client at another bus, where `udisks-fake --address ADDRESS` (in
olo-system) plays UDisks2 with four made-up disks (an NVMe system
disk, an empty SD card, a USB stick, an encrypted USB disk whose
passphrase is `secret`), real job progress and cancelling, and pipes
that drain at 256 MB/s for images. `cargo test -p olo-system --test
udisks_fake` runs everything against it when `OLO_TEST_BUS` names a
private bus (busd), and skips otherwise. Reading the real UDisks
read-only (`--list`) is fine.

## DiskUsage (apps/disk-usage)

Where a disk's space has gone, and getting it back: Haiku's DiskUsage
ring chart (`chart.rs`: the folder in the middle, what's in it in the
first ring by share of its size, their contents in rings outwards, five
deep; slices under ~0.7° left out; each first-ring item keeps a hue
outwards, lighter, files greyer) beside a list of the folder by size
whose bars wear the slices' colours, and DaisyDisk's Collector along the
bottom: drag rows or slices into it (or ⌘D, Item ▸ Add to Collector),
then Move to Trash or Delete… (confirmed) all at once. Click a slice to
go into it, the middle (or Up, ⌘↑, Backspace) to go back up. On the
GPUI kit (olo-ui): each slice is one `window.paint_sector` (the slices
one paint layer, the hairlines another), so hovering redraws the window
in about a millisecond and only when the slice under the pointer
changes; the chart's width is kept on device pixels so the list beside
it is too.

The start screen lists Home and each disk file system once
(`volumes.rs`, from /proc/self/mountinfo; statvfs for size and free
space). Scanning (`scan.rs`) is a worker thread that adds each folder to
a shared `Tree` in one write, so the chart and list grow as it goes
(the window sorts while it runs; the tree is sorted once at the end).
Sizes are allocated blocks, hard links count once, symlinks are never
followed, and it stays on one file system: `volumes::Mounts` crosses a
mount point only for another part of the same file system (a btrfs
`/home` under `/`), never `/proc`, `/tmp`, other disks or a bind mount
of what it already covers. Moving to the Trash (`olo_xdg::trash`) and
deleting run on a thread too; the tree drops what went
(`Tree::remove`). `on_upgrade` keeps the scanned folder, the folder
shown and the Collector; after a re-exec it scans again and goes back.
Test against a fixture home (`fallocate` files under /var/tmp) with
`tsession run NAME HOME=… target/debug/olo-disk-usage DIR`.

The freedesktop Trash helpers Tracker and DiskUsage share are
`olo_xdg::trash`: `trash_slot` reserves a name and writes the
.trashinfo, `trash_origin` reads it back, `move_to_trash` renames into
the Trash when on the same volume (Tracker copies across volumes with
progress instead). Tracker's Empty Trash is instant to see: the Trash's
`files` and `info` are swapped for empty folders (`renameat2`
`RENAME_EXCHANGE`, so the paths always exist and the Trash icon and any
Trash window change at once), and what was in them is deleted in
`Trash/.emptying…` with progress; cancelled or failed, what is left goes
back (`ops::put_back`); one cut short is finished by the next.

## Terminal (apps/terminal)

The shells don't run in Terminal: they run in **olo-termd**
(`src/termd.rs`, a second binary of the same package), one per display,
which the first Terminal starts (it daemonizes and holds
`$XDG_RUNTIME_DIR/olo-termd-<display>.lock`, with its pid in it). It
owns every PTY and keeps its own emulator per shell (the same `io::Feeder`
and image placement as Terminal, without the pixels), so it always knows
the screen. Terminal talks to it over
`$XDG_RUNTIME_DIR/olo-termd-<display>.sock` (`src/wire.rs`: framed JSON
and raw bytes; one connection per Terminal, `src/client.rs`): it spawns
or attaches a shell, sends input and resizes, and gets the output. On
attach, olo-termd sends a replay (`src/replay.rs`) that rebuilds the
screen and scrollback in a fresh emulator: both screens when a program
is on the alternate one, the modes it set (cursor keys, mouse reporting,
bracketed paste, kitty keyboard flags…), cursor shape and title. Images
(kitty, sixel, iTerm2) come back as kitty graphics: each image sent once
under the program's id, then put row by row in its cells; olo-termd
keeps them zlib-compressed (`Store::pack`). The PTY comes along (SCM_RIGHTS) so Terminal can see the
foreground program and its working directory. olo-termd answers the
program's questions (device attributes) only while no Terminal is
attached.

So: **Quit** (and closing the last window) says goodbye and leaves the
shells running; the next Terminal opens its windows as they were
(Restoring, below) and a window on each other shell not shown elsewhere
(a launch with arguments opens a new one as well). Closing a
window hangs up its shell. If Terminal **crashes**, olo-termd starts
it again after 2 s (up to 3 times a minute, only while the session and
its Wayland socket are there). An **upgrade** re-executes both in place:
Terminal keeps which shells were in which window (`save_state`), and
olo-termd hands its PTYs and replays to its new self through an
inherited file (`OLO_TERMD_RESTORE`); Terminal reconnects on its own.
A shell whose Terminal went without a goodbye (re-executing, or
crashed) is still listed as that Terminal's (`Shell::left_by`, its pid)
until one takes it, so the other Terminals, re-executing alongside it
after an app_server reload, leave it be while that process lives, and
a crashed one's are free to adopt (`reattach`'s other shells: the pid
is gone). Listed as no one's, each Terminal had opened a window for
every other's shells, which closed again as their owners came back:
twenty windows for five, each splitting a tile on its way.
(A Terminal from before olo-termd hands its shells' PTYs to its new
self, the old way; the new one passes them on to olo-termd, ADOPT.)
When the desktop session ends (its pid file, watched with a pidfd) or on
SIGTERM, olo-termd hangs up every shell and exits; it also exits when
it has no shells and no Terminal for 3 s, or when its runtime directory
has gone. If its socket is deleted from under it, it listens again (so a
Terminal can always find its shells); it only removes the socket while
the file there is still its own. Idle, it sleeps: the socket and its
folder are followed with inotify (`olo_kit_base::watch`), the session's
pid file too until it's there, a shell's first output since shells
were last kept arms the keep (`PRINTED`, `Ev::Printed`), and the loop
waits for the next event or the next thing due (a keep, a Terminal to
start again, the linger's end), never on a timer (it woke 3.5 times a
second; now not at all). Each screen is kept twice (in
olo-termd and in Terminal), so a full scrollback costs about twice
what it used to.

**On GPUI.** One `Entity<Terminal>` holds every window by Terminal's
own `ui::WindowId`; each window's view (`ui::Shell`) renders through it
(`Terminal::render`), and window work asked for inside an update (open,
close, activate, resize) is deferred through `ui::WinOps`. A screen is
a canvas painted from its session's emulator (`view::paint`): each row
kept as it was last drawn (`view::RowCache`: its cells, backgrounds,
glyphs and box-drawing shapes, and its glyphs' `GlyphRun` where the
atlas has them), laid out again only when its cells, selection or the
look (`RowKey`: metrics, grid, scheme, the program's colours) change.
Only the lines alacritty says changed are looked at (`Term::damage`,
taken by the paint); when the screen scrolls (output, the scrollback)
each row is found where it went, by its cells, so only the rows new to
the screen are laid out. The rows are painted in one layer
(`Window::paint_layer`): one place in the scene's order for them all,
not each glyph's own (2 µs a glyph, 1.4 ms a frame on a big screen at
1×); the cursor, links, colour emoji and images (cropped, under a
content mask) over them. A row is laid out only up to its last cell
that isn't blank, and is the same row at another width but for blanks
at its end (`same_cells`), so a resize lays out only what it changes.
Printable ASCII in a style's own font isn't shaped: each glyph comes
straight from the font (`TextSystem::glyph_for_char`, kept per style)
at its cell, which is what shaping a monospaced font's own characters
gives; the rest (other scripts, emoji, what the font lacks) is shaped
with fallback. A frame of typing at 1× on a 3456×2234 screen: 0.2 ms
of painting; a whole screen laid out afresh, 1.2 ms (6.2 shaped).

**Scrolling.** The normal screen's scrollback scrolls by the point, as
everything else that scrolls: fingers, their glide and a wheel's eased
notches move it as far as they go (`view::Smooth`, on the tab's `Pane`),
past its ends against GPUI's rubber band (`gpui::Elastic`), springing
back. alacritty's display offset is its whole rows; the rest, and the
stretch, slide the rows drawn down or up (on device pixels), the row
above drawn too when part of it shows (`Pane::shift`, which the mouse's
cells allow for). Output arriving, or typing (which scrolls to the end),
moves the display offset itself: the position follows it. Each pane
wears the kit's `SlimScrollBar` over its screen's right edge (its top
the history less the display offset, in lines; dragged or paged, the
offset is set and the smooth position follows, `Smooth::sync`), in place
of the thumb it drew while scrolled back. It says where
it is each frame (`Window::note_scroll`), so the bench measures it
(scroll-check: steps as far as the fingers, an even glide). A program
that takes the mouse gets it as wheel reports, and one on the alternate
screen (that asks) as arrow keys, a line at a time. The
shell's output asks for a frame (`Wakeup`), as do typing, the mouse
and the bell; nothing is drawn on a timer except an agent's spinner
while one spins. Keys come raw: ⌘ is Ctrl with the right Ctrl held in
the Mac profile (`gpui::right_control_held`), menu shortcuts go to the
menus first (`menu::shortcut_command`), typed text and input methods
through the view's `EntityInputHandler`. The window's titles are
retitled 400 ms after typing or output settles (`retitle_soon`), and
the windows remembered 2 s after a change (`remember_soon`), not by
polling.

**Restoring.** Terminal keeps its windows on disk as they change
(`remember_soon`: 2 s after a change drawn, if anything differs, and
before it quits), in
`$XDG_STATE_HOME/olo/terminal/windows.json`: each window's tabs in
order, which one shows, its size, group, Tile All Tabs (and its chosen groups), and each tab's
folder and whether it's hidden when tiled. A Terminal that starts afresh (not an upgrade, which has
`save_state`) opens those windows again: a shell olo-termd still has is
attached where it was, and one that has gone (the session ended, or the
machine was off) gets a new shell in its place (`Start::Restore`,
`Spawn::restore`), in its folder, showing first what the old one showed
and then a dim "Restored from DATE". olo-termd keeps each shell's
normal screen and scrollback for that (`replay::archive`: not the
alternate screen or modes), zlib'd in `shell-ID.z` beside
`windows.json`: every minute for shells that printed since, and all
of them when the session ends. The directory and files are the
person's alone (0700, 0600); a shell that exits forgets its file, Start
Over forgets them all, and unclaimed ones go after 30 days. As the
session ends olo-termd tells each Terminal (`ENDING`) before hanging
up, so it doesn't remember the tabs closing or start a new olo-termd;
Start Over says it isn't that (`Link::starting_over`).

**Tabs.** A window holds one or more shells (`TermWin::tabs`, each a
`Tab` with its own `Session`); its strip (the kit's `TabStrip`) always
shows, one tab or many, so a window doesn't change shape as tabs come
and go. Every tab has the window's grid,
so a program in a hidden tab lays itself out right; a hidden tab isn't
drawn (its Wakeup
waits until it shows), and its bell marks it with a dot. Messages from
the emulator carry the shell's id, not the window's, so a tab can move:
**Tabs ▸ Move Tab to New Window** (or the tab's context menu). New Tab
(⌘T) starts in the current tab's folder; ⌘W closes the tab, Close Window
the lot (asking if anything is running). ⌘[ / ⌘], ⌘1…⌘9, Ctrl+Tab /
Ctrl+Shift+Tab and Ctrl+Page Up / Page Down go through them (the shell
never sees those). `olo-terminal --tab [-w DIR]` opens a tab in the
Terminal window last in front on the workspace showing (Tracker's Open in
Terminal does), a new window if there is none. **Open in Tracker** (in
the tab's context menu, the terminal's and Terminal ▸) shows the tab's
working folder (its foreground program's, else the shell's, read from
`/proc/PID/cwd`) in Tracker. `save_state` keeps each window's tabs in order and
which one shows. Settings ▸ Tabs puts the strip **Along the Top** (the
default) or **Down the Side** (the strip's `.side()`; also the last item
of a tab's context menu), in `terminal.json` as `side_tabs`, with the
column's width (drag its edge) as `side_width`; switching keeps every
window its size and gives the grid the difference.

**Links** (`src/links.rs`). ⌘ held over a URL or a file's path
underlines it and ⌘-click opens it (the profile's command key: the
right Ctrl with the Mac's keys, `gpui::right_control_held()`, else
Ctrl): a URL in the
default browser, a file or folder in Tracker (`olo_ui::tracker::open`, a
file selected in its folder's window). Links are an OSC 8 hyperlink the
program wrote, else found in the text: URLs by pattern, paths (also
`path:line[:col]`, in backticks, brackets or Markdown) that exist
relative to the foreground program's directory, else the shell's. One a
program broke over rows itself is joined (`links::broken`: a piece ends
its row within 4 columns of the edge, the next is the first word of the
next row, indented at most 8; Claude Code wraps long URLs so), from any
piece. The link is looked up before the click is handled, so a ⌘-click
that focuses the window acts (its first frame knowing ⌘ is held), and
it opens even in a program that takes the mouse. Tested against a real
Claude Code reply in a test session (its HOME the person's, its
`CLAUDE*` variables unset), with a stand-in browser in the test HOME's
`mimeapps.list`.

**Groups** (`src/groups.rs`). A window's tabs can be in groups, as a
browser's: each a name and a colour (`TermWin::groups`, each tab's
`Tab::group` into them), its tabs side by side (`TermWin::tidy` keeps
them so, in the order they first come, and drops a group left with no
tab). Make one with Tabs ▸ Group ▸ New Group… or a tab's Add to Group ▸
New Group…; put a tab in one with Add to Group ▸ (name), take it out
with Remove from Group, or drag it: among a group's tabs it joins,
past its group's edge it leaves (the kit's `dropped_into`). A new tab
from a grouped one joins its group. Drag a group's chip to move the
whole group; click it for its menu (Rename Group…, New Tab in Group,
Save Group, Move Group to New Window, Ungroup, Close Group, Show All
Tabs, its repository's worktrees); Tabs ▸ Group has the same for the
group of the tab showing (`group_command`; commands carry the group's
index or the tab's shell). A tab or a group dragged out of the strip
and let go becomes a window under the pointer; a tab's menu also has
Move to New Window and Move to Window ▸ (Terminal's other windows,
named by one of their shells: `tab-to:SHELL:TARGET`). The window's
title is "group — tab" for the tab showing, set the moment a program
retitles its tab (`retitle`: Claude Code's status glyph in it), not at
the next frame. A tab in a git repository is in that repository's group
on its own (repos.rs `auto_group`): the group beside it, else one made
for it in the colour that repository's group has in another window;
never one elsewhere in the window (a tab doesn't jump on `cd`), nor
once the person took it out of a group (`Tab::own_grouping`); one that
leaves the repository leaves a group it joined on its own. `--tab -w
DIR` opens at the end of the group for DIR's repository, if one is
here. Save Group keeps the group and each of its tabs' folders in
`terminal-groups.json` (config dir); Open Saved Group shows its first
tab where it is open, else adds it to the window it was chosen in (a
shell in each folder); Forget Saved Group
drops it. Tile All Tabs (Tabs menu or the group's menu, ⌘⇧D) shows
every tab of the window at once, each a live pane with its title over
it: a row for each group, its tabs side by side across it (the tabs in
no group a row of their own, where the first of them is:
`TermWin::tile_rows`, `tile_in_rows`), and one group (or none) alone in
a grid (`tile` picks the columns that keep panes nearest a terminal's
shape, the last row spread across); each pane's shell sized to its pane.
In it a group's chip in the strip chooses groups (`TabStrip::
group_selection`, lit as Deskbar lights the workspace showing; agents
read it as `selected`): at first every group is tiled and every chip
lit (`tile_group_shown`); clicking one takes that group out (its chip
outlined) and clicking it again brings it back (`tile_groups_toggled`:
the last group tiled stays, and every group chosen is the same as
none). A tab's menu has Hide When Tiled (Show When Tiled once hidden;
the Tabs menu has it for the tab showing): its tab stays in the strip,
faded (`StripTab::faded`), and every tiling leaves it out
(`tile_rows_of`), its room going to the rest; picked in the strip, it
shows on its own till a tiled tab is picked (`TermWin::tiling`: tiles
only while the tab typed into is one of two or more; a lone tile has the
window, without a head). It's kept with the window (`hidden_tiled`). The
right button gives the group's menu, as a click does outside Tile All
Tabs. The chosen groups go by name (`tile_groups`, kept with the
window across restarts), and the tab typed into stays one that shows.
With several rows, the line between two rows and the heads of the row
under it drag the rows' heights (the pointer a row-resize one near the
line; a click there still picks the pane): the two rows grow and
shrink, each keeping its heads and two lines (`row_spans` gives each
row that least and shares the rest by weight; `drag_rows` moves weight
between the two), the others as they were. The weights go by the
row's group's name ("" for the tabs in none; `tile_heights`, kept with
the window), so a row keeps its share as others come and go. The right
button there gives the row's group's menu with Reset Row Heights
(`tile-reset-heights`, enabled once dragged), as the chip's has in Tile
All Tabs; agents find each divider as a `Splitter` named "Row Height:
GROUP" with its share as its value, and drag it;
each pane has a head (`pane_head_height`: a `STRIP_ROW` tall under a
strip along the top; beside a side strip `STRIP_SIDE_HEAD`, the same,
the strip's first row centred on it) with its icon and title on the
strip's quiet band, and at its right its group's chip as the strip
draws it (`strip::chip`), so it's plain which pane is in which group
without a line or a tint; its icon, title and chip are centred on the
band above the line under it (`snap_centred`, as near as the device
pixels allow); panes after the first mark themselves
`layout_review::own_grid`, since dividing the window puts them level
with the strip only by chance; and the panes not typed into are dimmed
so the one that is stands out, its head on the panel's ground with its
title in full ink and medium weight, as the strip's selected tab.
Clicking a pane makes it the one typed into, and the strip follows.
The screen keeps `view::PAD` (`WINDOW_INSET` × 6) between the text and
the window's edges, in the screen's own background
(`view::screen_colors`), so text never touches the frame or the strip.
The chrome is on one grid: the menu bar's first title, the tabs' and
pane heads' icons and the shell's text start on one column, 12 in from
the content's edge; tabs along the top and pane heads under them are
all one row, 26 (a side strip's rows are 24, its headings 20), and the
menu bar is one more of those rows (`menu::BAR_HEIGHT`, 26:
at 21 it was 31.5 pixels at 1.5× and looked cramped). Panes are tiled in device
pixels (`tile(area, n, ppp)`), a device line apart, so no seam opens
between them at 1.5×. The screen marks itself `layout_review::own_grid`:
its lines are a cell apart, not held level with the chrome.
⌘⇧D again goes back to one tab at a time. `save_state` keeps each
window's group and whether it shows all tabs across an upgrade. A new
⌘⇧ shortcut must also go in `view::SHORTCUTS`, or the shell gets the
Ctrl key too (⌘⇧D would send Ctrl+D and end the shell).

**Agents** (`src/agent.rs`). A thread looks at every tab (shown or not)
when a shell has printed (`session::on_output`, a `Nudge` every output
chunk pokes, coalesced), at most every 600 ms, and while a reading
settles; with nothing printed it sleeps: if the shell's foreground
process is Claude Code (its
native build under `…/claude/versions/`, or the npm package under node)
or Codex, it reads the bottom 30 lines of that tab's screen as a person
would. A question ("Do you want to…", "❯ 1. Yes", "esc to cancel") means
*needs your answer*; "esc to interrupt" means *working*; anything else is
*waiting for you*. A reading has to hold for three looks before it
counts as idle or gone, so the gap between steps isn't the turn ending.
The tab's icon wears the state (a turning arc, an orange "!", a green dot
once a finished turn hasn't been seen), the tab's accessible name and
the Tabs menu say it ("Claude Code: needs your answer"), and when an
agent the person isn't looking at starts asking, or finishes, Terminal
posts a notification (important for a question) whose Show brings that
tab and window forward. The badge and the notification go once the tab
is seen.

**Coding agents' context** (`src/agent_context.rs`; Preferences ▸
Terminal ▸ Coding Agents, off by default). A Claude Code or Codex in a
tab of a group whose tabs are in more than one place (a group per
feature, the feature spanning repositories) is told the group's name and
those repositories and worktrees (each one's path, which repository, its
branch), its own marked, nothing assumed about what they are for; a
group in one place, or no group, tells nothing. Through the agents' own
hooks, both of which take a hook's output as context: on, Terminal puts
`<its program> --agent-context` on `SessionStart` and `UserPromptSubmit`
in Claude Code's `settings.json` (`CLAUDE_CONFIG_DIR` or ~/.claude) and
Codex's `hooks.json` (`CODEX_HOME` or ~/.codex), each only where that
agent is set up, every other key as it was (`set_hook`); off, it takes
them out. Codex skips a hook until the person trusts it once
(`/hooks`; it says so at its start). While on, Terminal keeps its tabs
for the hook in `$XDG_RUNTIME_DIR/olo-terminal-groups-<display>.json`
(each tab's shell pid, group, folder and agent; every two seconds when
they change, with `remember`); off, it's removed. The hook
(`olo-terminal --agent-context`, before anything else in `main`) reads
the session on stdin (`session_id`, `cwd`, `hook_event_name`) and every
Terminal's file (tabs whose shells are gone left out), finds the tab:
Claude Code runs it under the tab's shell (an ancestor's pid), Codex in
its daemon (by the session's folder, a tab running an agent first, with
2.5 s for a tab just opened to be written), and prints the block; on
`UserPromptSubmit` only if it changed since it last told that session
(`$XDG_RUNTIME_DIR/olo-agent-context/<session>`), so a tab joining or
leaving the group is known from the next turn.

**Coding agents and Agent Screens** (`agent_context::SCREENS`;
Preferences ▸ Terminal ▸ Coding Agents, off by default).
- When on, the same hook tells a Claude Code or Codex in an Olo session
  (`OLO_SESSION`), in a tab or not, as it starts (`SessionStart`, which
  is also after a compaction):
  - to prefer an Agent Screen of its own for anything graphical, never
    the person's screen;
  - the few `olo-screen` commands it needs;
  - that web pages go in Olo's Chrome on the screen (`olo-screen run
    NAME olo-chrome URL`, in the screen's own profile), never a
    `google-chrome`/`chromium` it starts itself or the person's profile
    (Chrome would hand the window to the person's Chrome on their screen);
  - that the `olo-screens` skill has the rest.
- The words are `apps/screen/agent-context.md`, beside the skill
  (`apps/screen/SKILL.md`, whose Chrome section says the same), so the
  two are changed together.
- Where /tmp is held in memory (a tmpfs, `olo_system::env::in_memory`),
  the hook also says at the start, with the group context, where
  scratch goes (`SCRATCH`: a directory of the agent's own under
  /var/tmp, removed when done, never /tmp, and why).
- Terminal's file carries `screens` for the hook. It waits for the
  agent's tab (up to 2.5 s) only when the group or title settings need
  it: with this one alone, the hook answers in about 3 ms.

**Coding agents' tab titles** (`src/agent_title.rs`; Preferences ▸
Terminal ▸ Coding Agents, off by default). A Claude Code or Codex in a
tab gets a tool, `set_tab_title`, and is asked to keep the tab titled
with what it is working on: the hook above (installed while either
setting is on; the file says which, `Snapshot::groups`/`titles`) prints
the instructions at `SessionStart` and a one-line reminder with each
prompt. The tool is an MCP server the person's session serves over
loopback HTTP, `olo-terminal --mcp-service` (`src/title_service.rs`, one
of olo-session's `REAL_SHELL`), at `http://127.0.0.1:17893/mcp` (Token
Pool has 17891 and 17892): MCP's streamable HTTP, a JSON-RPC 2.0
request per POST answered with plain JSON (`initialize`, `tools/list`,
`tools/call`; notifications 202; GET and DELETE 405, as there is no
stream from the server and no session). Why HTTP: Claude Code reconnects
an HTTP or SSE server by itself, and never a stdio one (its own code:
`type !== "stdio"` before "attempting automatic reconnection"), so a
stdio server that went (three were ended by signals on 2026-11-06 while
their sessions went on) took the tool for the rest of the session until
the person reconnected it with /mcp. Over HTTP nothing is lost: the
service keeps no sessions (no `Mcp-Session-Id`), so any copy of it
answers any request the same, and each response closes its connection.
An upgrade (olo-session's SIGURG) stops it taking connections, lets the
requests under way finish, and execs the new version in place with the
listening socket handed over (`OLO_TERMINAL_MCP_LISTENER`,
`restore::exec_new_version_with`): what connects meanwhile waits in the
kernel's queue, so nobody is refused (`tests/title_service.rs`: four
threads asking through ten upgrades, every request answered by the same
process). Should it stop otherwise, olo-session starts it again, and an
agent's next call works (a call while it was away fails once with
ECONNREFUSED, the one after succeeds, in the same Claude Code session).
Only this user's programs may set a title (for `tools/call`, the
connection's other end looked up in `/proc/net/tcp`: its user, about
1 ms), and no web page is answered (an `Origin`, or a `Host` that isn't
loopback, is refused: DNS rebinding). Idle it costs nothing (asleep in
`poll`: 0.0% of a core, 0 wakeups/s, 8 MB); a request takes 1.5 ms
(median) from an agent's connecting to its answer, a `tools/call` about
10 ms (28 ms at the 99th percentile: finding the agent's process among
the user's). On, Terminal
names its URL in Claude Code's global `.claude.json`
(`mcpServers.olo-terminal`, `{"type": "http", "url": …}`, in
`CLAUDE_CONFIG_DIR` else the home folder; Claude Code rewrites that file
now and then, so it is read just before and checked after), allows the
tool in its `settings.json` (`mcp__olo-terminal__set_tab_title`, so it
isn't asked each time), and in Codex's `config.toml`
(`[mcp_servers.olo-terminal]`, `url = …`, through `toml_edit`, the rest
of the file as it was; Codex takes HTTP servers on loopback); off, it
takes them out. An agent sees it from its next start. The service finds
the tab from the process that asked (the socket's holder: agents'
processes, `claude`, `codex`, `node`, looked in first; every process
of the user's otherwise, about 25 to 45 ms more; only for `tools/call`): it and its ancestors, as the hook does (Claude Code
is under the tab's shell; Codex's daemon by the folder the agent passes
as `cwd`, else its own, the tab running the same agent first,
`agent_context::own_agent`), and hands the title to that tab's Terminal
on the tab's display (`olo-terminal --set-tab-title SHELL TITLE`
through the single-instance socket, `instance::forward_on`, which
Terminal listens on: the GPUI port had dropped `instance::listen`, so
no title and no second launch ever arrived). The snapshot and the
socket are found in `olo_xdg::runtime_dir()` (`XDG_RUNTIME_DIR`, else
/run/user/UID). `olo-terminal --mcp` still serves the same over stdin
and stdout (a line each way), for an agent whose settings named it
before Terminal rewrote them; it stays as it is through upgrades
(SIGURG ignored; olo-session leaves alone what an agent started) and
notes its life in `~/.local/state/olo/terminal-mcp.log`. Without the
tool, an agent sets the title from its shell: `olo-terminal --tab-title
WORDS…` (`agent_title::from_shell`, handled before anything is handed to
a running Terminal, so it never opens a window) finds the tab as the
service does, from its own ancestors (the agent's shell, the agent, the
tab's shell) and its folder, and prints what the tool would say (exit 1
outside a tab). The hook's words name it for when the tool isn't there,
since nothing in an agent brings the tool back by itself: Claude Code
keeps the MCP settings a session started with (one begun before
Terminal moved it to HTTP still has the stdio server, and starts it
again only through /mcp) and gives an HTTP server up after five tries
(`mce`, about 15 s of the server away); the hook is the installed
binary, so what it says reaches sessions already running from the next
prompt. `tools/dev/agent-title-check` drives it both
ways in a headless session: as Codex (a bare environment, outside the
tab, by folder) and as Claude Code (from the tab's shell, across the
server's file replaced and SIGURG). The tab shows it (`Tab::agent_title`,
one line, at most 60 characters) ahead of the program's own title until
the agent leaves the tab or sets an empty one; it is saved with the
window, and after a restart kept only if the tab's agent is seen again
within 5 s.

**Keep It Going** (Ben, 2026-11-06; `Tab::keep_going`, `KeepGoing`): on a
tab with a coding agent, its menu (and Tabs ▸ Keep It Going, which
agents reach through the commands socket, `tab-keep-going:SHELL`) has
Terminal say "please continue" to the agent once it has stopped: after
two minutes (`KEEP_GOING_QUIET`) with nothing at all changing on its
screen and nothing typed, its turn over (`State::Idle`: not while it
works, nor on a question, whose answer is the person's), at most once
an hour (`KEEP_GOING_EVERY`). Activity is the agent watcher's: when it
reads a tab after output (at most every 600 ms, as for the agent's
state), a change of the screen's `agent::fingerprint` is reported
(`agent::Activity`), with the clocks that count by themselves taken out
first (Codex's "Pursuing goal (17h 52m)", which goes on every minute).
A timer of the tab's own sleeps until the quiet would be long enough,
so nothing runs while it waits; the words go in as typed, the Return
150 ms after (all at once, a TUI may take it as pasted text). The tab's
icon carries a blue disc with a white "go on" triangle at its bottom
corner while it's on; its name says so to screen readers and agents.
Kept with the window (when it last spoke too, so a restart doesn't make
the hour start again); `OLO_TERMINAL_KEEP_GOING_SECS` shortens the wait
for a test. Tried headless with a stand-in named `claude` (an idle
screen, a goal clock ticking every 2 s): one "please continue" after
the quiet, none again within the hour, back after Terminal quit and
started again.

**Repositories** (`src/git.rs`, `src/repos.rs`), in process through
libgit2 (git2, vendored; no `git` or `gh`). A thread (`git::Watcher`)
looks at each tab's folder (its foreground program's, else its shell's)
when a shell has printed (a `cd`, a `git switch`) or a file it follows
changes (inotify, `olo_kit_base::watch`: the worktree's `HEAD`, the
repository's `config`, its `HEAD`, `worktrees`, `refs/heads` and
`packed-refs`, and `.git` in a folder in no repository, for a `git
init`), at most once a second, and reads only what changed; it reports
the tab's
repository (`git::Repo`: name, main worktree, the worktree it's in,
linked or not, branch, GitHub remotes) and, for each repository a tab is
in, its worktrees and local branches when `worktrees/*/HEAD`,
`refs/heads` or `packed-refs` change (the windows drawn again, as their
menus list them). GitHub's thread sleeps until the branches wanted
change or its next ask is due. Idle, Terminal wakes for none of this (2
wakeups in 30 s, GPUI's own, from 3 a second).
* A window whose tabs are all in one repository becomes its group on its
  own (`Group::auto`, named after it, in the colour of another window of
  it if there is one); the automatic group follows the tabs (gone when
  none is in it, another repository's when they all move there). Ungroup
  keeps a window out of this (`own_grouping`, kept with the windows); a
  group the person names in a repository is still its group
  (`Group::repo`), and Name Group… suggests the repository's name.
* The group's menu, and Tabs ▸ Repository for agents and the keyboard,
  list the repository's worktrees (the tab's checked; choosing one opens
  a tab in it in this window) and **New Worktree…**: a name, which is a
  folder and a branch both (`git::valid_name`), made in Preferences ▸
  Terminal's location, `<location>/<repository>/<name>`, on a new branch
  from the main worktree's, or the branch of that name if it has one and
  no worktree has it checked out (the dialog says which, or why not, as
  you type). `git::add` makes it as `git worktree add` does (git's record
  in `<common>/worktrees/<name>`, the `.git` file) then checks the files
  out 400 at a time on a thread, so the dialog shows "Checking out N of
  M files" and Stop (or closing it) ends it after the batch, leaving
  nothing (nor a branch it made). Git LFS files stay pointers (no
  `git-lfs` to run). Done, it opens in a new tab of the group. A
  `--tab -w DIR` launch (Tracker's Open Terminal Here) in a repository
  goes to the window that's its group, on this workspace, rather than the
  front one.
* A tab in a linked worktree has **Rename Worktree…** (its folder moves,
  beside where it is; its branch takes the new name if it had the old
  one; git's record is rewritten; its shells stay in it, though a shell's
  `$PWD` keeps the old name until it `cd`s) and **Remove Worktree…** (an
  alert counts the files changed and not committed, on a thread; the
  worktree's tabs close, then its folder and record go; its branch stays,
  and its repository's folder in the worktrees location if now empty).
* GitHub: another thread asks GitHub's API for the newest pull request
  from each branch the tabs are on (`origin`'s owner's branch, in
  `origin` and `upstream`, so a fork's is found), open ones first, and
  its check runs; every minute with a token (`olo_system::github`,
  below), every five without (and then only public repositories answer:
  a tab in a repository on GitHub gets **Connect GitHub for Pull
  Requests…** in its menu and Tabs ▸ Repository, which opens Preferences
  ▸ Integrations).
  The tab wears `style::pull_request_mark` as its strip badge (green
  open, grey draft, purple merged, red closed; the branch's commit
  filled green, red or orange as its checks pass, fail or run), its
  accessible name says it ("pull request #42 open, checks running"), and
  its menu and Tabs ▸ Repository have **Open Pull Request #42**.
  `OLO_GITHUB_API` points it at another API (tests use a fake one on
  localhost).

**Terminal ▸ Terminal Daemon…** (`src/daemon.rs`) shows olo-termd's
process and uptime and its shells (whether each is shown here, in
another Terminal, or nowhere; double-click one to show it, bringing its tab forward), refreshed
every 2 s off the UI thread. **Restart** re-executes it in place (shells
kept); **Start Over…** (after an alert) ends it, by its pid, with SIGTERM
then SIGKILL, starts a fresh one, and opens a new window once the old
shells' windows have closed. One that isn't answering can only start
over.

Testing: olo-termd lives in the session's runtime directory, so a
headless session gets its own. Kill only the Terminal or olo-termd
you started, found by `XDG_RUNTIME_DIR` in `/proc/PID/environ`.
`cargo test -p olo-terminal` covers the wire format (with descriptor
passing) and replays (scrollback, `clear`, the alternate screen, modes,
images).

Keys: physical Ctrl goes to the shell. ⌘ reaches Terminal as Ctrl+Shift
(see keymap.rs) on the right Ctrl, which the kit marks as
`Modifiers::command` (see "Raw keys"); ⌘ with one of Terminal's own
shortcuts in the keyboard profile (`keyboard::is_own_shortcut`, its
`terminal.*` keys in `olo_protocol::app_keys`) is Terminal's, and any
other goes to the shell as Ctrl+key, so ⌘R searches history like Ctrl+R.
With Windows' keys Ctrl is the command key itself: Terminal's are
Ctrl+Shift ones (Ctrl+Shift+C copies), as in Windows Terminal.
A physical Ctrl+Shift chord isn't `command`, so it is always the
shell's: Ctrl+_ (readline's undo, Ctrl+Shift+-), Ctrl+^ and Ctrl+@,
while ⌘- is Smaller. Bigger and Smaller keep each showing window's
columns and rows, resizing it as far as app_server lets it (held by a
tile or a zoom, it keeps its size that way); one away on another
workspace or hidden keeps its size and fits its grid to the new text
when it is back (`set_text_size`, `is_shown`), rather than jump
then. The menus read ⌘N, ⌘T, ⌘+, as pressed. ⌥←/⌥→ send ESC b / ESC f
(a word back/forward in readline, zsh and fish), except to a program
using the kitty keyboard protocol (Claude Code), which gets the
protocol's `CSI 1;3 D` / `C` and reads ESC b as Escape then b. (The
arrows must stay legacy `CSI 1;mod X` keys there, so `view::kitty_code`
has no code for them.)

**Colours** (`src/colors.rs`, Settings ▸ Colors). Match Appearance,
Always Light or Always Dark picks which of two schemes shows; Light
Colors offers Olo Light, Catppuccin Latte and Catppuccin Frappé, Dark
Colors Olo Dark, Catppuccin Macchiato and Mocha (`SchemeId::LIGHT` /
`DARK`). Frappé, the lightest of Catppuccin's dark flavours, is the
light default and Macchiato the dark one (`SchemeId::light` / `dark`).
A Catppuccin scheme draws as its own ports do (`Scheme::catppuccin`:
Text on Base, Rosewater cursor, its 16 ANSI colours); the choices are
`Settings::light_colors` / `dark_colors`.

Text is kept legible whatever background a program had in mind
(`colors::legible`, as iTerm's minimum contrast): a cell's text is at
least WCAG's 4.5:1 from its background (`MIN_CONTRAST`), moved toward
black or white just as far as it takes, its hue kept, so a dark
terminal's pale greys and blues, its bright white, or the default text
on a band a program drew (a diff's dark green, a composer's grey) read
in light mode, and faint text (SGR 2, mixed two thirds toward the
background) stays faint. Not moved: text the colour of its cell (hidden
on purpose) and characters that are pictures (box drawing, blocks,
Powerline's, `colors::graphic`). Per cell after faint and inverse;
14 ns a cell legible already, 56 ns moved, and the last pair is kept
(colours come in runs), on rows rebuilt only.

Programs learn the colours by asking: OSC 4/10/11 are answered with the
scheme's own (`session.rs`), and light or dark with DEC mode 2031 (as
contour, Ghostty and kitty do, and Neovim reads): `CSI ? 996 n` is
answered `CSI ? 997 ; 1 n` (dark) or `; 2 n` (light), by the scheme's
background (`Scheme::dark`); a program that turned the mode on
(`CSI ? 2031 h`) is sent the same unasked when the tab's colours turn
from light to dark or back (the look's appearance, View ▸ Colors), so
it picks its own again without being restarted. The io scanner notes
the mode on its way past (no pass of its own over the output), and
olo-termd keeps it in a shell's replay.

**xdg-open in Terminal's shells** (`src/olo_open.rs`, `olo-open`):
Terminal links `xdg-open` to olo-open in the same bin directory as
`sudo` (below) and sets `BROWSER` to it, so a link or file a command
opens (`gh auth login`, `git web--browse`) goes to the app Olo opens it
with (Preferences ▸ Default Apps, `olo_xdg::open`), started on its own,
and the command carries on at once: the system's xdg-open ran Firefox as
the command's child, and `gh auth login` waited for it to quit before
finishing. Exit statuses as xdg-open's (2: no such file, 4: failed).

**sudo in a window** (`src/sudo.rs`, `src/olo_sudo.rs`, `src/sudo_wire.rs`).
A coding agent's commands have no terminal to type a password into
(Claude Code runs each in a session of its own, reading /dev/null), so
sudo alone can only fail there. Terminal links `sudo` to **olo-sudo** (a
third binary of the package, beside it) in
`$XDG_RUNTIME_DIR/olo-terminal-bin-<display>/`, puts that first on its
shells' `PATH` and sets `OLO_TERMINAL_SUDO` to the socket it listens on,
`olo-terminal-sudo-<display>.sock` (`sudo::shell_env`; only shells
started since, so after an upgrade, new tabs). olo-sudo runs `sudo -n -v`
first: with a credential cached or none needed, the command just runs.
Otherwise it asks Terminal, which opens an alert over the window of the
tab it came from (found by walking up from the pid to a shell it
owns) naming who asks (a coding agent by name, `agent.rs`) and the
command, and relays sudo's own PAM conversation from `sudo -S -v`:
the fingerprint first where `/etc/pam.d/sudo` has pam_fprintd and a
finger is enrolled, then the password, its errors ("Sorry, try again.")
in the alert. Cancel, Escape or the close box say no (exit 1, "sudo:
not run: cancelled in Terminal's password window"). Then the command runs on the credential sudo just
cached: the terminal's, or with no terminal its parent's (olo-sudo stays
the parent). Settings ▸ Ask for sudo Passwords in a Window
(`sudo_window`, on) covers sudo typed at a prompt; with it off those ask
on the terminal as ever, but a command with no terminal still gets the
window. `-n` (never ask: how agents run sudo so as never to wait) is let
in while Deskbar approves without asking (`Ask::Begin { quiet }`:
Terminal gives sudo the password, no window either way, and says no
when not approving, as `-n` does). Whatever else olo-sudo can't help with (`-S`, `-A`, `-k`, `-K`,
`-h`, `-V`, no Terminal) goes straight to sudo. Safety: the password goes
only to a sudo that is root's and set-user-ID, the first on `PATH`
(then /usr/bin, /bin) that isn't olo-sudo itself; Terminal answers only a peer of the same uid whose
executable is olo-sudo (`SO_PEERCRED`, `/proc/PID/exe`). Limits: with
the credential cache off (`timestamp_timeout=0` in sudoers) a command
with no terminal fails after the window; with the fingerprint first,
the password field waits until pam_fprintd gives up (as in the polkit
agent); with a terminal, pam_fprintd's "Place your finger…" also prints
there. Testing: debug builds take `OLO_SUDO_REAL` as the sudo to run, so
a test stands in a script; never test against the real sudo or
fingerprint reader. `cargo test -p olo-terminal sudo`.

## The polkit agent (apps/polkit-agent, olo_system::polkit)

Asks for a password when a program needs an administrator (timedated,
logind, NetworkManager, …). `olo-session` starts it only in a real
`--tty-udev` session, with `OLO_REAL_SESSION=1` (cleared for nested
and headless sessions); without that it refuses to run, because it
registers with polkitd for the whole **login session** and must never take
over the prompts of the session it was started from. It stays resident
(`olo_ui::stay_resident`) and opens a BAlert-style window per request
(`widgets::AlertFrame`). Proving identity runs polkit's setuid
`polkit-agent-helper-1` (the only thing polkitd accepts an answer from):
the cookie goes on its stdin, and its PAM conversation (`PAM_TEXT_INFO`
from fprintd for Touch ID, then `PAM_PROMPT_ECHO_OFF` for the password)
is relayed to the window; a failure starts it again for another try.

Test it with `tsession run NAME olo-polkit-agent --self-test ACTION`:
it serves only its own process and asks polkitd about ACTION with
`CheckAuthorization` (which changes nothing), printing the outcome to
its log. **Cancel; never type the user's password, and only check
actions, never perform them.** `OLO_POLKIT_HELPER` points at a fake
helper (a script speaking polkit-agent-helper-1's lines: `read cookie`,
`PAM_PROMPT_ECHO_OFF Password: `, `read pw`, `SUCCESS` or `FAILURE`):
for unit tests, and with `--self-test` to drive the whole dialog
(wrong answer, retry, success) with no PAM at all; polkitd still says
"not authorized", as only the real helper can vouch for the cookie.
The self-test ends once polkitd has answered and its window is gone.

**The keyring's prompts** (`olo_system::keyring_prompt`, the agent's
`keyring.rs`): gnome-keyring-daemon asks for the login keyring's password
(to unlock it for gh, a browser, …) through a "system prompter" on the
session bus. D-Bus would start GTK's gcr-prompter for that; the agent owns
`org.gnome.keyring.SystemPrompter` (and `PrivatePrompter`) itself from
the start of the session, so the question is an Olo alert (key icon,
Password, or Password and Confirm for a new keyring, the daemon's check
box if any). The protocol is gcr's: `BeginPrompting` → our
`PromptReady("")` with a Diffie-Hellman public key → `PerformPrompt`
(type, properties, the daemon's key) → `PromptReady("yes"|"no")` with
the password encrypted (1536-bit IKE group, HKDF-SHA256, AES-128-CBC,
PKCS#7: `Exchange`), and `StopPrompting` → `PromptDone`. Each property
value is a variant inside the a{sv}'s variant. Calls are answered from
zbus's message stream before anything follows from them (gcr's client
breaks if `PromptReady` overtakes the `PerformPrompt` reply). A wrong
password comes back as another `PerformPrompt` with a `warning`; the
window stays "Checking…" in between. One prompt at a time; a caller that
leaves the bus takes its prompts with it, and if the agent goes, the
keyring dismisses the request instead of hanging. The shared secret is
padded to the prime's length before HKDF, as gcr's and libsecret's
`egg_dh_gen_secret` write it (unpadded, one exchange in 256, whose
secret starts with a zero byte, derived another key and a right password
read as wrong). `keyring_prompt::ask_password(props, wrong, check)` is
the other side, for a program of Olo's own that keeps secrets
(olo-secrets): it asks through the same prompter, as gnome-keyring does,
again with `wrong` as the warning till `check` says yes; blocking.

Test it on a private bus with a throwaway daemon, never the session's:
start a bus (`dbus-broker-launch` with a listening socket handed over as
`LISTEN_FDS`, a permissive config) and `gnome-keyring-daemon --foreground
--components=secrets` with its own `HOME` and `XDG_RUNTIME_DIR`; then
`KEYRING_TEST_BUS=unix:path=… cargo test -p olo-system real_daemon --
--ignored` (makes a keyring, wrong then right password, cancel), or
`olo-polkit-agent --keyring-test unix:path=…` in a headless session and
a Secret Service client calling `Unlock` and `Prompt`.

## Programs' secrets (apps/secrets)

`olo-secrets` is Olo's own Secret Service (`org.freedesktop.secrets`, the
freedesktop API 0.2): with Preferences ▸ Security ▸ Programs' Secrets on
(`security.passwords.secret_service`, on by default), the secrets
Chrome, Slack, NetworkManager's applets, `gh` and Olo itself keep through
libsecret live in Olo's store rather than GNOME Keyring's, and the
Secret portal is Olo's too. olo-session
starts it in a real session (`REAL_SHELL`, and on an upgrade that adds
it); while off it waits on app_server's settings events (no CPU, no
wakeups) and follows the switch live.

- **The store** (`store.rs`): `$XDG_CONFIG_HOME/olo/secrets.store`,
  "OLOS", a format byte, a 24-byte nonce, the JSON sealed with
  XChaCha20-Poly1305 (the header as associated data), written atomically
  at 0600 after each change. Collections (`login`, the `default` alias's,
  made first; others by `CreateCollection`) of items (label, attributes,
  secret, content type, times); the `session` collection in memory only.
  Its key is 32 random bytes, sealed by the login password beside the
  store (`login_key.rs`: `secrets.login`, "OLOL", a format byte, a
  nonce, the key sealed with XChaCha20-Poly1305 under the key the
  password gives, `olo_login_key::login_key`: Argon2id, 64 MiB, 3
  passes, salted by the uid and /etc/machine-id, 99 ms here), kept in
  GNOME Keyring's login keyring too where it was made there
  (`olo_system::secrets::SECRETS_KEY`; a kept secret, so Backup carries
  it, and the store with it as a vault: "Programs' Secrets"), and for
  the rest of the login in the kernel's session keyring (`kernel.rs`:
  `olo-secrets:` and the store's path, so a test's key never meets the
  real one), so olo-secrets started again (an upgrade, a crash) opens it
  without asking anything.
- **The login password opens it** (`crates/pam-keyring`,
  `pam_olo_keyring.so`, in olo-login's password service in
  `pam_gnome_keyring`'s place: `-auth optional` after `password-auth`,
  `-session optional` after `pam_keyinit`; tools/install-login.sh puts it
  in /usr/lib64/security). At authentication it derives the key the
  typed password gives and keeps it with the PAM handle; once the
  session opens it puts it in the login's session keyring
  (`olo-secrets:login-key`, the user's, gone in ten minutes, and with
  the login). It never fails a login and never reads the person's files;
  with no password (a face, a fingerprint, the greeter) it does nothing.
  olo-secrets takes the key once (read, then invalidated) and opens the
  sealed store key with it (`key`). With none from PAM (another login
  screen, a face, a fingerprint) or the password changed since it was
  sealed, it takes GNOME Keyring's copy if GNOME Keyring is answering
  (another login screen's PAM unlocked it), else asks for the password
  through Olo's prompter (`ask_password`: the one used before, if it
  changed) until it opens the seal; then it seals again with this
  login's. The first time (nothing sealed yet) it takes GNOME Keyring's
  copy, or makes a key for a new store. Tested in a real PAM run
  (`tests/pam.rs`, private services and a session keyring of the test's
  own): the typed password's key arrives in the session keyring, and a
  stack without a password leaves nothing.
- **Taking over** (at login, or switched on): the key, the store, then,
  when GNOME Keyring is answering or its keyrings
  (`$XDG_DATA_HOME/keyrings/*.keyring`) changed since the last copy (a
  GNOME session since: D-Bus starts it, and it asks for its password
  through Olo's prompter; neither, and nothing of GNOME's starts),
  every GNOME Keyring item new or changed since the last copy
  (`Data::imported`: identity, its `Modified`; identity is collection,
  label and attributes without `xdg:schema`, which GNOME Keyring adds to
  an item that had none once it has read it back from disk), each locked
  GNOME Keyring collection unlocked first through Olo's prompter (not
  when started again: it waits for the next login). Then it asks for the
  bus name with no flags, so it waits in line (GNOME Keyring lets nobody
  replace it: `RequestName` with `REPLACE_EXISTING` says it exists), and
  tells GNOME Keyring to quit over its control socket (`gnome.rs`: op 3,
  `GKD_CONTROL_OP_QUIT`), the name coming to olo-secrets as it goes
  (waited for as `NameAcquired`, up to 5 s). GNOME Keyring's other
  parts (pkcs11) go with it; SSH is Passwords' agent or gcr's.
- **Serving** (`service.rs`): the service, collections (also at
  `…/aliases/NAME`), items, sessions and prompts as the spec has them,
  with their properties and signals. Sessions are `plain` and
  `dh-ietf1024-sha256-aes128-cbc-pkcs7` (`session.rs`: RFC 2409's 1024-bit
  group, the shared secret padded to 128 bytes, HKDF-SHA256 to an AES-128
  key, CBC with PKCS#7 and the IV as the secret's parameters: libsecret's
  default, so Chrome's), each only for its own caller and closed when it
  leaves the bus (a match on `NameOwnerChanged` with no new owner). Every
  collection is unlocked once the store is open; `Lock` locks one, and
  `Unlock` gives a prompt asking for the account password in Olo's
  keyring window (`ask_password`), checked by PAM. Changes made while
  serving are noted (`Data::changed`, `deleted`).
- **The Secret portal** (`service.rs`, `Portal`:
  `org.freedesktop.impl.portal.Secret` at `/org/freedesktop/portal/
  desktop`, version 1): each program's own secret, which sandboxed
  programs, and Chrome and Electron apps (Slack), ask the portal for
  first. Kept in `login` as GNOME Keyring's portal keeps it (`app_id`
  and `xdg:schema` `org.freedesktop.portal.Secret`, "Application key for
  …"), so those copied from it are what each program gets; made (64
  random bytes) the first time a program asks, then written to the
  portal's file descriptor, which is closed. olo-session describes it
  to the portal as `olo-secrets.portal` (`DBusName=org.freedesktop.
  secrets`: whoever answers for the keyring, so GNOME Keyring, which
  serves the same, when Olo's is off) and routes Secret to it. Tried
  through the real frontend on a test bus: the same 64 bytes both times.
- **Handing back** (switched off): the name given up, then what changed
  while Olo answered written into GNOME Keyring (D-Bus starts it again;
  it asks for the password to unlock through Olo's prompter) and what was
  deleted deleted there; the kernel's copy of the key forgotten.
- **As GNOME Keyring answers** (Ben, 2026-11-06: gh lost its token
  just after he switched): an item made without `xdg:schema` gets
  `org.freedesktop.Secret.Generic`, as GNOME Keyring gives it; and
  `CreateItem`'s `replace` replaces an item that has every one of the new
  one's attributes (GNOME's `gkm_secret_fields_match`), not only one with
  exactly them. Matching exactly, gh's token written again (go-keyring
  sends `service` and `username` only) went in beside the copy of
  GNOME's, which had its `xdg:schema`, and go-keyring reads the first it
  finds: the old one. go-keyring (gh's) reads and writes
  `…/collection/login` whenever the service lists it, whatever the
  default alias says. And `Item.GetSecret` replies with one struct,
  `(oayays)`, as the spec has it (a tuple returned from a zbus method is
  several out arguments, `oayays`, though the introspection says the
  struct; Python and libsecret, which reads through `GetSecrets`, took
  it, go-keyring couldn't, and gh found no token). The take-over test
  writes and reads as go-keyring does and checks the reply's signature.
- **Mending** (`Store::heal`, every take-over, so every upgrade, which
  starts it again): every item given GNOME's `xdg:schema`; a second login
  collection (an id `login` but for its case: GNOME Keyring's "Login",
  which Olo's own client made beside `login` on a computer with no
  default alias, 2026-11-04) merged into `login`, aliases following, and
  GNOME's "Login" copied into `login` from then on; in each collection
  one item per set of attributes, the newest, kept first (as go-keyring
  takes the first found). What it did is in the status (`healed`) and
  Olo Doctor says so ("Programs' secrets are in one login keyring"); if
  two login keyrings are ever listed, Doctor's Merge Them starts
  olo-secrets again. With GNOME Keyring answering, Doctor says when it
  has a second "Login" keyring. Handing back writes the login
  collection's items into GNOME's `login` (not the default alias's,
  which may be "Login"), making `login` the default again there; and
  `olo_system::secrets` (Passwords' key) makes an existing `login` the
  default rather than creating another.
- **Status** for Preferences and agents: `$XDG_RUNTIME_DIR/olo-secrets.json`
  (serving, since, each collection's count, the programs with secrets by
  their `application` attribute or schema, the last copy, collections
  left locked, the last error), followed by Preferences' box with an
  inotify watch; `olo-secrets --status` prints it.

Measured (release, a private bus): idle while serving, 0.0% of a core
and no wakeups (`tools/dev/cost`, 5 s); a lookup through libsecret
(`secret-tool`, its start-up included) 7.6 ms against GNOME Keyring's
8.0. Tests: `cargo test -p olo-secrets` (the store sealed and opened,
replacing by attributes, a libsecret-style client and the service
agreeing over 64 DH sessions, the kernel keyring); and against a real
GNOME Keyring with libsecret's own client, on a private bus that never
reaches the person's (`tools/dev/private-keyring start`, which now uses
`dbus-daemon` when busd isn't installed and keeps the display out):
`KEYRING_TEST_BUS=$DBUS_SESSION_BUS_ADDRESS cargo test -p olo-secrets
--test takeover -- --ignored` takes over (3 copied, GNOME Keyring gone),
answers 300 `secret-tool lookup`s (encrypted sessions), stores, replaces,
searches and clears, locks and unlocks through a test prompter
(`OLO_SECRETS_TEST_PASSWORD`, never the account's), hands back (1
written, 1 deleted; GNOME Keyring started again and unlocked through the
prompter), takes over again copying only what changed, and starts again
without GNOME Keyring or a prompt. `OLO_SECRETS_SERVE=1|stdin`,
`OLO_SECRETS_KEYFILE` and `OLO_SECRETS_STORE` are for tests.

## Notifications (apps/notifications)

Olo's own apps notify through the kit: `olo_ui::notify::Notifier::
new(olo_ui::proxy(cx), "Tracker", "olo-tracker")`, then `show(None,
Note::new("Copied").body(…).action("default", "Show"))` → a `NoteId`;
`show(Some(id), …)` updates it in place (`Note::progress(title, 0.4)`
for a job's bar: transient and silent), `close(id)`. Kinds are
`NoteKind::{Information, Important, Error, Progress}`. The bus is used
on the notifier's own thread; what the user does comes back to
the app's `message_handler` as `NoteEvent::Action { id, key, token }`
(bring a window forward with `Window::activate_window_with_token`) or
`NoteEvent::Closed`.
A program without windows (a helper app_server starts) uses
`Notifier::with_events(app, entry, |e: NoteEvent| …)` instead; the
callback runs on the notifier's thread. Tracker's "Finished copying…" (jobs
of 10 s or more; `OLO_TRACKER_NOTIFY_AFTER_MS` in debug builds) uses it.

A note can say how it looks, so each app's cards are its own:
`.accent([r, g, b])` (the app's colour: its badge, its monogram when
it has no icon, its progress bar; never a band or edge), `.subtitle(…)` (a line
under the title: from whom, a channel, an album), `.badge("PR #42")` (a
pill beside the title), `.field(label, value)` (a little table),
`.image(path)` (a picture shown large: a screenshot, a photo),
`.style(NoteStyle::Message)` (the sender's picture round, the body in a
balloon) or `Media` (the artwork large), and `.size(CardSize::Small |
Medium | Large)` (how big its card is in the history; `Auto` lets it
choose). Each is a `Notify` hint (`olo_protocol::notifications::hint`:
`x-olo-accent` "#rrggbb", `x-olo-subtitle`, `x-olo-badge`, `x-olo-fields`
as `a(ss)` or `a{ss}`, `x-olo-image`, `x-olo-style`, `x-olo-size`,
`x-olo-source` for where in the app it came from, `x-olo-type`), so any
program can send them (`notify-send -h string:x-olo-accent:#e5534b …`)
and other services ignore them.

`olo-notifications` serves `org.freedesktop.Notifications` (spec 1.2)
on the session bus, so `notify-send`, browsers, chat and mail clients and
anything else using libnotify reach the user. The real session starts it
(`REAL_SHELL` in olo-session; an upgrade starts it in a running session
too); a nested or headless one never does, since it would take the user's
notifications.

It is a GPUI program (`olo-ui`): `Notifications`, an entity, keeps the
service's state (the cards on screen, the history, the settings) and
the views draw from it: banners.rs the layer surface's, history.rs the
History window's. Nothing polls: a card's expiry is one timer for the
next to go, the clock's minute a timer to the next minute (fading, Do
Not Disturb's schedule, snoozes due during a suspend), and the settings
and the screen-sharing state files are watched (`olo_kit_base::watch`).
Pictures (pictures.rs) are made for the place they're drawn, at its
device size, on a worker (`pictures::get`: a cover cut from the middle
or fitted whole), kept, and the windows drawn again as each comes.

- server.rs: the zbus interface on its own thread. `Notify` becomes a
  `Note`: the body's markup kept as spans (markup.rs: `<b>`, `<i>`, `<u>`,
  `<a href>`, `<img alt>`, `<br>`, entities; unknown tags shown as text),
  `image-data` decoded to RGBA, and the hints read: `urgency`,
  `resident`, `transient`, `category`, `desktop-entry`, `value`
  (progress), `sound-file` / `sound-name` / `suppress-sound`,
  `x-canonical-private-synchronous` (and `synchronous`,
  `x-dunst-stack-tag`: the same tag from the same app replaces the last
  one), KDE's `inline-reply` action and `x-kde-reply-placeholder-text`,
  and our own `x-olo-*`, into the note's `NoteLook` (an `im.*` note is a
  `Message` unless it says otherwise; `x-olo-image`'s size is read on the
  bus thread). Each note has a `Kind` after Haiku's
  BNotification: information, important (`im.*` and `call` categories),
  error (critical: stays) and progress. The timeout: -1 is 6 s, 0,
  errors and notes waiting for a reply stay until dismissed. The UI
  answers with `Bus::closed` (reason 1 expired, 2 dismissed, 3 closed by
  the app), `Bus::action` after `Bus::activation_token`, and
  `Bus::replied` (`NotificationReplied`). If another service owns the
  name it waits and retries every 30 s rather than taking it. (zbus's
  `Builder::name` treats "taken" as success; `serve` checks the
  RequestName reply itself.)
- web.rs: websites' notes, through Chrome or Chromium, made the site's
  own (`web::adopt`, as each arrives). Chrome sends a site's as its own
  (its name and desktop entry) with the site as a link heading the body,
  the page's icon as the picture and critical urgency for
  `requireInteraction`. Here the site becomes the app: the link comes
  off the body into `look.source`; a site we know (`SITES`: Gmail,
  Google Calendar, GitHub, WhatsApp, Slack, Spotify, …) gets its name,
  its colour and its style (a chat site's are `Message`s, a music
  player's `Media`); one we don't is
  named by its host. `note.via` keeps the browser: Preferences falls back
  to the browser's choices for a site with none of its own
  (`NotificationSettings::app_via`), "stay until dismissed" stays rather
  than being an error, Chrome's Settings button reads Site Settings and
  is left off banners and cards (an open row in the history has it),
  and with no icon of its
  own the site's mark is a monogram tile (`web::tile`: the last word's
  first letter on its colour, or on one its name picks).
- A banner's actions (`banners::lay_buttons`, when the card is
  measured, so its height counts them): side by side from the right at
  their own widths (`widgets::button_width`), as in an alert, when
  they fit Deskbar's width; else one above another, each the card's
  width, the last at the bottom, and a label cut short with "…" only if
  even that is too narrow. (Before, a label wider than its share pushed
  its button past the card's edge.)
- look.rs: what a note shows, the same on screen and in the history's
  cards: its picture (square; round for a `Message`; large for `Media`),
  the title, a line with its badge and subtitle, the body (in a balloon
  for a message), its fields as a little table, the large image under it
  all and a progress bar. `look::lay` measures in a `Room` (width, title
  and body rows, picture sizes) so a stack or a grid is arranged before
  `look::paint` draws.
- main.rs: one layer surface (Top layer, anchored bottom right, flush
  with the edge) opened for the first card and closed with the last,
  resized to the stack. Cards are `metrics::DESKBAR_WIDTH` wide, so they
  stack in the Deskbar's column, usually free space. Up
  to four cards show, as many as fit in the screen's height (at least
  one), grouped by app under a title row with the app's mark and name
  (and a close box for all of a group's), oldest first; the rest wait.
  No bands, edges or rules: an app's accent shows only in its badge,
  monogram and progress bar, and an error's title leads with a small
  stop sign (`look::paint_stop`: `style::stop_mark` level with the
  capitals, the title laid out with exactly the sign's room before its
  first row and the whole width under it, `look::lay_led`), as do its rows
  in the history; progress shows a `widgets::level_bar`; links in the
  body open with `open::open`. A card
  is laid out by look.rs; without a picture the text takes the width,
  the group's title row already showing whose it is: `paint_mark` draws
  the named icon (ours for common theme names, `named_icons`), the app's
  desktop-entry icon, else a monogram for a site or an app with an accent,
  else the generic application icon (the History's rows the same: the
  app's mark, never a warning or info sign beside a title that already
  has its stop sign). Finding an app's desktop entry can read every one
  on the system (milliseconds each time; it held the History's Cards to
  30 fps), so `app_icons` looks each app up once, on a worker
  (`warm_app_icons`: the history's apps at start, a note's as it comes),
  and keeps the answer. Action buttons share the width when they must.
  Banner `seconds` 0 ("Never" in Preferences) shows only errors.
  A click on a card runs its `default` action: the card asks GPUI
  for an activation token from that click (`Window::activation_token`) and
  sends `ActivationToken`, `ActionInvoked`, then `NotificationClosed`, so
  the app may bring its window forward (app_server lets a token one of
  our programs asked for, with an input serial, raise another app's
  window). With no default action the click brings the app's window
  forward itself, over IPC, by desktop entry or name. Action buttons
  along the bottom; a reply field and Send when it can be answered (the
  layer takes the keyboard on demand only while one shows,
  `Window::set_layer_keyboard`); the close box dismisses. Buttons and the
  reply field sit over the card's own click area `.occlude()`d, so a
  click on them isn't the card's too. While the pointer is over
  a card it does not expire. Sounds (sound.rs): the note's `sound-file`
  or `sound-name` (looked up in the freedesktop sound theme), else
  `message-new-instant` for important notes and `dialog-warning` for
  errors, played on a thread through olo-preview's `audio::Player`.
- store.rs: the history, `$XDG_STATE_HOME/olo/notifications/
  history.json` (the newest 200; saved atomically off the UI thread),
  each note's picture beside it in `pictures/ID.png` (scaled to 96 px,
  written off the UI thread; pictures nothing names go at start-up).
  Every note not `transient` goes in, unread. Read means the person has
  seen to it: clicked (on screen or in the history), acted on, or
  dismissed; a card that just expires stays unread. An expired card
  stays "open" there (its actions still work) until the app closes it.
  After a restart nothing is open, and ids carry on after the
  history's. With fading on, a note goes once it has faded away
  (`History::fade_out`, checked every tick, never one still on screen;
  its app hears it expired). The window draws a note at
  `fade_opacity(strength)`, never fainter than 0.3, so the last of its
  fade is still legible (full again selected or under the pointer).
- history.rs: the History window (`--history`, or Deskbar's bell),
  unread first under an *Unread* heading (with its count and Mark All
  Read), then the rest, newest first in each, in one of four views
  (View menu, ⌘1–⌘4, `view` in `notifications.json`): *List* under day
  headings, *By App* under folding app headings (with Dismiss per app),
  *Cards*, *Conversations* with the same app and title stacked into one
  with a count. A row is two lines: a 16 px icon, the title with the app
  and time on its line, one line of body. Unread wears a dot (in the
  keyboard's blue) and a bold title; read, the title
  goes regular and the dot goes. A click selects and reads it (the row
  opens to its whole body, the rest of its conversation and its actions
  as buttons, the default one always Open, or Show APP once it closed,
  then Dismiss, which forgets it and the rest of its conversation); what
  was read while the window is open keeps its place until it closes, so
  nothing jumps under the pointer. A double click or Return runs the
  default action while the note is open, else brings the app forward.
  Edit ▸ Mark as Read/Unread (⌘U, `toggle-read`) and Mark All as Read
  (`read-all`). × or Delete forgets one; Clear All forgets all but those
  on screen, pinned or snoozed. A right click on a row or card opens its
  menu (`entry_menu`): Open, Mark as Read/Unread, Pin, Snooze ▸, Forget.
  With fading on, each row and card is painted at its
  `strength` (`with_element_opacity`), the newest strongest, back to full
  while selected or under the pointer. *Cards* packs cards of different
  sizes with `olo_kit_base::masonry`: columns of at least 176 px; a card
  spans two when its app asks (`CardSize::Large`) or, left to choose,
  for a large picture, three fields or more, or a long body, and so does
  the newest unread one when there are three columns or more; `Small`
  keeps to one line of body. Each card is a `style::card` (selected, in
  the selection's colours): a quiet head of its app's mark and name with
  the time on the right (an unread dot before it, in the keyboard's
  blue, not the tab's yellow; a close box in their place under the
  pointer), its face laid out by look.rs, its actions as
  `Button::small`s; one click on a card runs its default action (or
  brings the app forward), as a banner does, and selects it; ⌘- or
  Shift-click only selects it; the arrow keys move by where
  cards are (`masonry::neighbour`). The search field (⌘F) takes every
  word against app, title and body; Escape clears it. For agents each
  note is a `ListItem` named by its title (with `row`/`rows`), its text
  as labels. The window, search and folds come back after an upgrade
  (`on_upgrade`). The list is laid out once for what it shows
  (`ListLaid`: the notes in their lines, each row's or card's text laid
  out, kept by entry) and kept while it scrolls or the pointer moves
  over it; a change to the notes, the view, the selection or the search
  (the app entity notifying), a command, a key or a click, another width,
  scale or palette lays it out again. Laid out every frame, 200 notes
  cost a scroll frame 2–4 ms; kept, 1.3–1.8.
- Pin and Snooze (`note_menu`: Edit ▸ Pin/Unpin and Snooze ▸ for the
  selected note, and the menu of a right click on a banner, row or
  card). A pinned note (`Entry::pinned`) stays put: it never fades
  (`fade_out` passes it; the history draws it at full strength and says
  "Pinned" after its time), Clear All, a banner group's close box and By
  App's Dismiss leave it, the history keeps it beyond its 200, and its
  banner stays on screen until closed (no `until`), back again after a
  restart or an upgrade (`load` keeps `shown` only for pinned notes; the
  header says "Pinned" when all of a group's are). Unpinned, a banner
  times out as usual. Snoozing (`Entry::snoozed_until`, seconds since
  the epoch: 15 minutes, an hour, three hours, or nine tomorrow morning
  by `mktime`, so a change of clocks doesn't move it) takes the note off
  the screen and out of the history and the unread count (the status
  line counts it as snoozed) until then; `wake_snoozed` (each tick: a
  timer for each, and the clock's minute, which catches one due during
  a suspend) brings it back as new: unread, its time now, a banner
  (unless quiet, or banners are off for its app), its actions only if
  its app still has it open. A replacement from the app keeps it pinned
  or snoozed (a snoozed one's update waits, silent).
- Settings: `olo_protocol::NotificationSettings` in
  `notifications.json` (config dir): Do Not Disturb (`dnd`, `dnd_until`,
  a daily `schedule` in local minutes), banner `seconds`, the history's
  `fade` (on by default) over `fade_minutes` (60: `strength(age)` is
  1 − t^1.6, slow at first, then faster, to nothing; `faded(age)` once
  it's gone), and per-app
  `banners` / `sounds` / `history` by app name (a site's falling back to
  its browser's): Preferences' Applications table, its All Apps row over
  the list setting a column for every app at once (checked when every
  app's is, mixed while they differ: `CheckBox::mixed`). Preferences'
  History box shows the fade as a strip from
  Now to gone. The service watches the
  file and reads it again as it changes (Preferences ▸ Notifications and
  Deskbar write it). While quiet nothing pops up or sounds but errors.
  It is quiet too while the screen is shared (`while_sharing`, on unless
  turned off: "While sharing the screen" in Preferences' Do Not Disturb
  box), from olo-portal's `SharingState`, the file watched: then errors
  wait in the history as well, and the banners on screen when the share
  starts go into it as if they had timed out.
  It publishes `NotificationState` (`unread`, `quiet`, and `apps`: each
  app with notes unread and in sight, by desktop entry and name, a
  website's as its browser's, with its newest's `(time, id)`) to
  `$XDG_RUNTIME_DIR/olo-notifications-$WAYLAND_DISPLAY.json`, which
  Deskbar's bell reads twice a second (the badge; struck through while
  quiet; right-click for Do Not Disturb), and its application rows (the
  dots, below).
- gtk.rs: GLib's `org.gtk.Notifications` (`AddNotification`,
  `RemoveNotification`), owned beside the standard name so GApplication
  apps (`g_application_send_notification`) use it, as under GNOME. Notes
  are keyed by (app id, the app's id) and share the id counter; `title`,
  `body` / `markup-body`, a serialized GIcon (themed, file, or bytes
  decoded with `image`), `priority` (high: important, urgent: error),
  `category`, `default-action` and app-action `buttons` (keys `gtk-N`;
  `win.*` ones are dropped, as GLib only runs `app.*`). An action calls
  `org.freedesktop.Application.ActivateAction` (or `Activate`) on the
  app itself, on a thread, with the click's activation token in the
  platform data; D-Bus starts the app if it has quit. There is no closed
  signal. Flatpak apps reach us through the portal: olo-portal's
  Notification sends a `Notify` (the app's name, and its id as the
  `desktop-entry` hint) and hands the action back to the portal when we
  signal `ActionInvoked` ("Screen sharing", the portal's interfaces).
- For agents: `com.olodesktop.Notifications1` at `/com/olodesktop/Notifications`
  (`List` → JSON: `unread`, `quiet` and each note with its `look`, `via`,
  `pinned`, `snoozed_until` and `strength`; `Invoke`, `Dismiss`,
  `SetRead(id, read)`, `MarkAllRead`, `SetPinned(id, pinned)`,
  `Snooze(id, minutes)`, `ClearHistory`, `ShowHistory`,
  `SetDoNotDisturb`). Each
  returns once the UI has done it (it answers through a oneshot), so a
  `List` straight after sees the change. The same from the command line
  (cli.rs): `olo-notifications [--bus A] --list [--json] | --history |
  --invoke ID [ACTION] | --dismiss ID | --read ID|all | --unread ID |
  --pin ID | --unpin ID | --snooze ID MINUTES | --clear | --dnd
  on|off|MINUTES`; `--list` says which are pinned and until when each is
  snoozed.

Test it on a private bus: `busd -a unix:path=/tmp/olo-dev/NAME/bus.sock`
(`cargo install busd`; Aurora's dbus-broker-launch needs systemd), then
`tsession run NAME olo-notifications --bus unix:path=…` and send with
`gdbus call -a unix:path=… -d org.freedesktop.Notifications -o
/org/freedesktop/Notifications -m org.freedesktop.Notifications.Notify --
APP 0 ICON SUMMARY BODY '["default","Open"]' '{}' -1`; `dbus-monitor
--address … "type='signal',interface='org.freedesktop.Notifications'"`
shows the signals (released busd refuses `gdbus monitor`'s match rule,
and Chrome's, which put spaces after the commas: Chrome then falls back
to popups of its own. `tools/dev/busd-lenient` builds a busd whose
parser takes them and prints its path; use it for anything that
subscribes, Chrome included. The real session's dbus-broker takes them
as they are).

Chrome's notifications come here as they are: a site's
`new Notification(…)` and an extension's `chrome.notifications` arrive
over `org.freedesktop.Notifications` (Chrome's name, its icon, the site
as a link in the body, critical while `requireInteraction`); a click is
the page's `onclick`, and Chrome's Settings button its action. Run it with `XDG_STATE_HOME` (and the
session's `XDG_CONFIG_HOME`) under the test directory so the history and
settings stay out of yours, and `PULSE_SERVER` at an `olo-fakesound`
socket to hear what it plays. A Deskbar or Preferences in the test
session needs `DBUS_SESSION_BUS_ADDRESS` set to the private bus too, or
its bell's click reaches your own notification service. Unit tests:
`cargo test -p olo-notifications`.

## The command palette (apps/palette)

On the GPUI kit (olo-ui). Released, it opens in 28 ms from the forward
to the first frame (egui's 47), typing's frames take at most 2 ms once
its glyphs are drawn (5.3 the first time; egui's 5–6.6 each time), and
shown it holds 113.6 MB (egui's 154.1); hidden, nothing wakes it.

⌘Space (`Shortcut::Palette`, the profile's: ⊞S on Windows, Alt+Space
on BeOS; Find moved to ⌥⌘Space) or a tap of the Super key (if chosen in
the keyboard profile; Windows' does) has app_server start
`olo-palette --toggle`; the running copy (it stays resident, found
through `instance`) takes the toggle, so it opens at once from the second
time on. Re-executed by an upgrade, it starts again with the arguments it
first had (`--toggle`, or a window menu's), so it saves a state
(`olo_ui::on_upgrade`) only to know it was: restored, it doesn't open
itself (it did, on the first display, while app_server brought the
others back). It opens a full-screen transparent overlay layer that takes the
keyboard, on the display in use (the focused window's, else the
pointer's), with the BeOS panel a fifth of the way down (over a soft
shadow on device pixels, `style::soft_shadow`, as the shortcuts sheet's;
the query's capitals on the field's middle, its text where the
results' titles start; each row's icon and words on one middle on the
device grid, the subtitle and kind on the title's baseline): a
click outside the panel, Escape or ⌘Space again
put it away.

* **Before opening**, a worker asks app_server for the windows and the
  settings, so the window in front is known before the overlay takes the
  keyboard (app_server's focus is then the overlay). Shortcuts are done
  to that window through `Request::RunShortcut { id, window }`; text
  editing's keys (`sheet::text_keys`: each line of the sheet's Text
  card, a command for each key, "Move to the end of the line" ⌘→, and
  with ⇧ "Select to …") through `Request::PressKeys` (`Chord::physical`,
  pressed once the overlay has let the keyboard go, through the profile
  as the person's own; nothing else is asked first, as an `Activate`
  then moved the keyboard after them and they were lost).
* **Commands or files**: at first everything but files and folders;
  a tap of Ctrl on its own (not ⌘, which comes as the right Ctrl)
  switches to files and folders only, and back (`files_mode`,
  `modifiers`), the placeholder saying what it finds and a `KeyHint`
  at the field's right ("Ctrl Files", "Ctrl Commands") what the tap
  finds instead; a path copied or typed is offered either way. The
  field is a little taller than its line on the device grid (it sets
  its text by the capitals' middle, so a field just a line tall cut the
  letters' tops at 1.25×).
* **Sources** (`sources.rs`): applications (ours, then desktop entries),
  "Go to ~/src/olo" for a path copied (offered first, before anything
  is typed) or typed whole (`sources::go_to`: absolute, `~/…`, a
  `file://` URI's, offered when there's something there; Tracker shows
  it, `Action::Reveal`: a folder in its window, a file selected in its
  folder's; the clipboard is read with `paste_as` once the overlay has
  the keyboard, as its offer comes with that, and the path looked for
  on a worker, as a typed one is, so a program slow to give what it
  copied or a slow disk never holds the palette up), preference panels
  and every setting in them (`PREFERENCE_SETTINGS`: "Pinch to zoom —
  Preferences ▸ Touchpad & Mouse ▸ Gestures ▸ Scrolling"; a choice or a
  short name is titled with its box, "Theme: Dark"; found too by what
  it's called elsewhere, `KNOWN_AS`: "night shift", "palm rejection", a
  test keeping each naming a setting that's there; chosen, Preferences
  opens on it, lit), light/dark appearance, app_server's shortcuts with
  their current keys (`keys_for`, shown as on a Mac, so "tile" lists every
  tiling shortcut and its keys), open windows, the front app's menu
  commands with their keys (`olo_ui::commands`, asked of the process
  that owns the front window; disabled items are shown dimmed), Passwords'
  items (`olo_keychain::Request::Search` while unlocked; choosing one
  is `Request::Show`), files under home by name (`sources::Index`: an
  `ignore` walk on start, at most 400,000, then each folder followed,
  `olo_ui::watch`, and only one that changed listed again, so a new file
  is found at once; past an eighth of the kernel's inotify watches it is
  walked again when asked and five minutes old instead; matched on their
  own thread for the newest query only, which sleeps until a query or a
  change), sums (`calc.rs`; Return copies)
  and, last, a web search (Google) in the web browser.
* **Ranking** (`rank.rs`, `model.rs`): every word typed must match the
  title (start, word start, anywhere) or the item's keywords, or as
  initials ("nw"); what matches is ranked by a tiny model, as brush's
  suggestions are (`olo_complete`'s kernels and trainer: 16 features →
  16 hidden units (ReLU) → a score, in the processor's SIMD, never a
  GPU). Its features (`model::FEATURE_NAMES`): how well it matches and
  whether the title starts with or is what's typed, its kind, whether
  it can be done now, and from the person's choices how often and how
  lately it was chosen, at this hour (±1) and on this weekday, after
  typing what's typed now, with the app in front now, whether it was
  the last thing chosen, how often its kind is chosen, whether its app
  is open, and the title's and the typing's lengths. Its weights start
  written by hand (`model::baseline`) so that, knowing nothing, it ranks
  as the palette's formula did (`sources::ranked`, the tests' reference:
  `the_model_untrained_ranks_as_before`). Every choice of a match is
  kept (`~/.local/state/olo/palette/choices.jsonl`, with what was
  typed, the item's key, kind and the app in front; menu commands and
  windows by their app, `model::key_of`) and is an example (the 24
  best ranked, the chosen among them, `examples.jsonl`, the newest
  1000); after each, the model is tuned on a worker (8 passes, Adam
  pulled toward where it started, `model::PLAN`: 5.7 ms on a full 1000,
  AVX2), the weights kept (`weights.json`) only if they rank the newest
  tenth of the choices at least as well. Per keystroke it costs what
  the formula did for most searches (103 µs either way for "end" over
  614 things) and 34 µs more for one letter matching nearly all of them
  (116 against 82). `olo-palette --model` prints what it weighs, how
  many choices and examples it has, and whether it's tuned. An open
  window is also found by its app's name and comes just before the app
  itself (switching beats opening another), the one in front last.
  Nothing typed lists what was chosen lately (`~/.config/olo/
  palette.json`).
* **Keys**: ↑/↓ and Page Up/Down move, Return chooses, the pointer
  hovers and clicks. ⌘Return (or ⌘-click) on a file opens it in Tracker
  instead (selected in its folder); while ⌘ is held the selected file's
  row says so. Actions run on a worker thread.
* **The window menu** (`window_menu.rs`): a right-click on a window's
  yellow tab has app_server (`wm.rs` `window_menu`) start
  `olo-palette --window-menu ID X Y OUTPUT`, the point on that output.
  A worker asks app_server for the window's title and workspace, the
  workspace count and the displays; the palette then opens a clear
  full-screen overlay on that output and pops an ordinary kit menu at
  the point: Hide, Zoom, Back to Its Own Size, Center, Tile ▸ (halves,
  quarters, thirds), Move to Workspace ▸ (every display's workspaces,
  left to right as Deskbar's heading menus list them, named as Deskbar
  names them, "D1 Workspace 2"; its own display's with their keys and a
  check; another's is `MoveToWorkspace { id, index, output }`), the
  next/previous display (with more than one), Move to Agent Screen ▸
  (each screen running and a new one; "Moving windows to and from
  screens"), Open in Tracker (when the window's program says
  what it shows: `commands::path`, asked of its pid with its
  `WindowInfo::surface`), Send to Back (BeOS's right-click on the tab,
  `Request::SendToBack`), then Close and Quit APP.
  Close and Quit are the app's own, as its keys press them:
  - A kit app's come from that window's menus (`commands::menus_in(pid,
    surface)`) and are chosen there (`commands::invoke_in`), with their
    labels and keys: Terminal's Close Tab with a tab of several closes
    that tab alone. Whatever the app asks first, it still asks (Terminal's
    "still running", StyledEdit's "Save changes").
  - Another program's Close closes the window (`Request::Close`). Its
    Quit closes each of its windows, as Deskbar's Close All does. Both
    show the profile's keys for every app's Close and Quit
    (`app_keys::app_key`: ⌘W and ⌘Q on a Mac, Ctrl+W and Ctrl+Q on
    Windows, Alt+W and Alt+Q on BeOS).

  The app is named as Deskbar names it (`olo_xdg::apps::app_name`). The
  other items are the shortcuts', done
  through `RunShortcut { id, window }`, so the menu, the keys and the
  palette always agree, and each shows the person's own keys for it
  (`shortcuts::keys_for` with their `ShortcutSettings`); agents read and
  choose from it as from any popup. Choosing, a click outside or Escape puts the overlay away.
  Its popup asks for its grab with no press of its own (the press was
  app_server's, on the tab) and often while the button is still down,
  so app_server lets a popup whose root is the `window-menu` layer take
  the pointer from that press (`XdgShellHandler::grab`): refused, it ran
  without a grab and its clear layer outlived it when put away with
  Escape, taking the next click. And when a popup goes, its root (the
  window, or a layer) is sent a frame callback at once
  (`popup_destroyed`): the kit draws on frames, and a still screen
  makes none, so a menu's window otherwise showed the menu gone only at
  the next change on screen.

## The emoji picker (apps/emoji-picker, app_server insert.rs)

`olo-emoji-picker` puts an emoji into the field that had the keyboard,
in any app. ⌃⌘Space (Windows' keys: ⊞.; BeOS': ⌃⌘Space;
`Shortcut::EmojiPicker`, "emoji-picker") starts `olo-emoji-picker
--toggle`, which hands the toggle to the running copy (resident, as the
palette is); the same overlay (a full-output layer, the keyboard its
own). The panel: a search field; a tab a group (and Frequently Used,
once something's picked), a cell wide each, the one showing at the
grid's top lit; the grid (ten 40-pt cells a row, headings between
groups, as many rows as fit up to six, scrolled by the pixel); a status
line with the selected emoji big, its name and shortcode (or the
model's reason), and the six skin tones as raised hands, the chosen one
sunken and kept. Arrows walk the grid (up and down a column), Page
Up/Down by its height, Tab and ⇧Tab go from group to group, Return or a
click puts the selected one in, Escape or a click outside puts the
picker away. Agents read every cell as a Button named for its emoji
(its glyph as the value, selected or not), the tabs as Tabs, the tones
as RadioButtons, the headings and the status line as text.

The emoji are `emojis`' (Unicode 18: name, group, shortcodes, skin
tones), with CLDR's English keywords from `emoji` (Unicode 13.1, so
newer ones have their names only), less those the colour font doesn't
draw as one glyph (`data::font_check`: Noto Color Emoji found with
fontdb as GPUI finds it, each base emoji shaped with
`olo_emoji::ColorFont::glyph`), loaded on a thread at start. Search
(`data::search`, as you type, 0.1–0.3 ms a key): every word typed must
match one of the emoji's words: the same (100, its name's own words 10
more), its start (60, less what's left), inside it (30, four letters or
more), or its letters in order within a word starting with the same
letter and at most two longer (12: "thmbs"); the name typed whole 60
more; four less for each name word past the query's and a little for a
longer name ("red heart" before "heart with arrow"), and up to 10 more
for the most used emoji (`COMMON`, Unicode's frequency list), what was
picked lately more again. Picked emoji are counted in
`~/.config/olo/emoji.json` with the tone.

⌘↩ (Ctrl+Enter on a PC: the command key reaches apps as Ctrl) asks the
model chosen in Preferences ▸ AI ▸ Emoji (`AiSettings::emoji`: on,
Astra 6, low effort) for five emoji for what's typed (`ai::suggest`,
crates/ai as Smart Tiling uses it, on a thread): the status line says
"Asking Astra 6… N s" (ticking only while asked), then they show first,
"Suggested for “…”", each with its reason; Unicode's emoji only, each
once (`ai::read`); a new search or Escape drops a question still out.
Test it with a fake model: `$T run NAME OLO_AI_BASE_URL=http://127.0.0.1:PORT
olo-emoji-picker`, the fake answering `POST /v1/responses` with
`{"status":"completed","output":[{"type":"message","content":[{"type":
"output_text","text":"{\"emoji\":[{\"emoji\":\"👏\",\"why\":…}]}"}]}]}`.

Its emoji are drawn ahead when it opens (`Window::warm_text`: what was
picked lately first, then the groups), kept for the next opening, and
let go ten minutes after it's put away (`FORGET`, with
`forget_kept_glyphs`): at 2× they're 17 MB of its private memory
(`Private_Dirty`: 5.6 MB never shown, 38 MB hidden after, the same
however often it opens; the emoji font it checks against is mapped,
not read). Released, at 1×, 1.5× and 2×,
its frames take at most 1.5 ms opening, typing, walked, scrolled and
opened again (bench's `emoji`).

**Putting text in** (`Request::InsertText { text }`, app_server
insert.rs): any text, not only what a keyboard types, into the focused
field, as an input method would. A field that speaks text-input-v3
(GPUI, GTK, Chrome, Qt) takes it whole (`commit_string`, then `done`):
app_server offers to be the input method (smithay's
`set_compositor_input_method`, which sends `enter`), the field turns on
(`TextInputActivation::activated`, then `text_input_on`), the text goes
in, and the offer is withdrawn. A window whose field doesn't turn on
within 150 ms (`TURN_ON`: a terminal, a toolkit without it) gets the
text as keys on a keymap made for it (`inject_text_keysyms`: each
character its own keycode, the seat's keymap back after). It waits up
to a second (`HANDBACK`) while the keyboard is still the asking
process's (the picker's panel going away), never goes into the lock
screen, and the reply comes once the text is in (or with why not), so
whatever the asker does next comes after it. Tested: é😀👍🏽 into
StyledEdit, Chrome 156 and GTK's text editor (as keys, with
`GTK_IM_MODULE=gtk-im-context-simple`).

**Pressing keys for someone** (`Request::PressKeys { keys }`, insert.rs
`press_keys`): evdev codes pressed together and let go through the
keyboard profile (`press_physical`, as a keyboard's go: ⌘→ is the
person's ⌘→, which app_server's keymap makes End for a program that
isn't Olo's), once the keyboard has left the asking process, a second at
most, never on the lock screen; the reply once they're pressed. The
command palette's text keys use it.

## Other toolkits' programs (app_server toolkits.rs, state.rs; olo_theme::toolkit)

Programs not built on the kit take Olo's look as far as their toolkit
allows. `toolkits.rs` publishes the palette where GTK 3, GTK 4,
libadwaita and Chrome read it (GSettings through dconf, the Olo and
Olo-dark GTK themes, GTK 4's user stylesheet, BeOS's close-left,
zoom-right title buttons); see its module comment. The GSettings keys
are `olo_theme::toolkit::desktop_settings`, which olo-portal's Settings
serves too (sandboxed programs, and those asking the portal), so the
two say the same.

**dconf is Olo's** (`apps/dconf`, `olo-dconf`). Programs read GSettings
from dconf's database themselves (`~/.config/dconf/user`, mapped) and
ask `ca.desrt.dconf` on the session bus to change it; olo-dconf answers
for it in GNOME's dconf-service's place, with the same interface and the
same file, so nothing that reads it changes, and a GNOME session later
reads what was set under Olo. `ca.desrt.dconf.Writer` at
`/ca/desrt/dconf/Writer/NAME`: `Change(ay) → s` takes a change as
clients send it (a GVariant `a{smv}`, read by its framing alone in
`changeset.rs`, so each value is stored as the bytes the program sent,
never decoded and encoded again), makes it in the database as it is on
disk then (read again each time, so nothing another writer did is
lost; a folder path with no value resets the folder), writes the file
whole (GLib's GVDB format, `gvdb.rs`, as dconf's writer makes it: every
folder up to "/" an `L` item listing its children, each item keyed by
what it adds to its parent's key, one bucket per item, no bloom
filter), renames it into place with its mode kept, sets the readers'
shared flag (`$XDG_RUNTIME_DIR/dconf/NAME`'s first byte, then removes
it: each reader opens the file again), answers with a tag, and only
then signals `Notify(prefix, paths, tag)`, so the writer knows its tag
first. `Init()` makes an empty database if there's none. olo-session
names it for D-Bus activation while an Olo session runs, before
app_server starts (`graphical::name_dconf`: app_server writes the
toolkits' look as it starts, and the bus had started GNOME's;
`graphical::take_dconf` again with the rest: `$XDG_RUNTIME_DIR/dbus-1/services/
ca.desrt.dconf.service`, which the bus reads before `~/.local/share`'s
and the system's: tried with dbus-broker 37, the same name in all
three, and the runtime folder's was the one started; gone with the
session's `stop` or the login, so a GNOME session has GNOME's), has the
bus read its services again, and stops `dconf.service` if GNOME's was
started (by its unit's state, `olo_system::env::user_unit_active`: just
started, it hadn't taken its name yet, and a check by the name missed
it): the next change starts Olo's. It goes after ten minutes with
nothing asked, giving up its name before answering what came meanwhile
(the next change starts it again). Checked with GNOME's own tools:
`olo-dconf --rewrite IN OUT` (a database written again as olo-dconf
writes it) of a copy of a real database (69 keys) gives the same
`dconf dump /`, byte for byte; through the service on a test bus,
`dconf write`, `read`, `list`, `reset -f` of a folder and `gsettings
set`/`get` work, and a running `dconf watch /` and `gsettings monitor`
see each change. A write (the `dconf` command run each time) takes
29 to 30 ms against GNOME's 37 ms; idle it costs nothing (0.0% of a
core, 0 wakeups/s), in 5.3 MB against GNOME's 6.8 MB.

**No gvfs.** GIO (GLib's files: GTK and GNOME programs, Chrome) loads
GNOME's gvfs when it's installed, which starts gvfsd, gvfsd-fuse,
gvfsd-metadata and volume monitors on the session bus. What olo-session
starts gets `GIO_USE_VFS=local` and `GIO_USE_VOLUME_MONITOR=unix`
(`olo_xdg::GIO_LOCAL`, set in `set_session_env`, and by
`olo_xdg::upgrade_env` for a session started on an older build as its
programs re-exec), so GIO uses its own local files (trash and recent
files are GIO's own there) and the system's mounts, and none of gvfs
starts; network folders, phones and cameras are Olo's Tracker's. It's
never exported to systemd or D-Bus activation, which keep it after
logging out, so a GNOME login afterwards has gvfs. Services systemd
starts while an Olo session runs get the same from a drop-in for all
services in the runtime unit folder
(`$XDG_RUNTIME_DIR/systemd/user/service.d/olo-gio.conf`, written before
the session's target starts, gone with `stop` or the login): at login
the target starts uresourced, a GLib program, and GLib started gvfs for
it the same second (a fresh Fedora 45); and what was started before the
session is stopped as it starts (`stop_gvfs`). (Checked on a test
bus that can start services: with them, `gio info trash:///` says
"Operation not supported" and no gvfs process starts.) Fonts are hinted as
the kit hints them (`font-hinting` slight, `font-antialiasing`
grayscale). The pointer: programs asking for one by name
(cursor-shape-v1: GTK 4, Chrome and Electron, Qt 6) get app_server's;
for those that draw their own (GTK 3, SDL, Java), `xcursor.rs` writes
the same Haiku cursors into the `Olo` theme's `cursors` (every CSS
cursor name at each scale from 1× to 4×, the old X names linked to
them; drawn again only when its `X-Olo-Cursors` version is raised),
named in `cursor-theme` and `cursor-size` (22, `olo_xdg::CURSOR_SIZE`)
and in olo-session's `XCURSOR_THEME` and `XCURSOR_SIZE`. The icons:
`icon_theme.rs` makes `Olo` (`~/.local/share/icons/Olo`, named in
`icon-theme`) a freedesktop icon theme of Olo's own pictures in the look
in use, linked from `data/icons` or the look's (`olo_xdg::own_icon_exact`)
at every size Olo has, for the freedesktop names of places (folder,
home, desktop, trash), kinds of file, devices, applications and their
categories, alerts and the session's actions (`ICONS`); everything else
comes from Adwaita and hicolor, which it inherits. It is remade when the
look, Olo's data folder or the cursors' version changes (`index.theme`
records them).

What an Olo session says it is (`XDG_CURRENT_DESKTOP`, set by
olo-session, `olo_xdg::CURRENT_DESKTOP`) is `Olo:GNOME`: Olo, then the
desktop whose settings it serves. Qt picks its platform theme by that
name and knows only the big desktops; for a name it doesn't know (plain
`Olo`) it takes its generic theme, light, in its own font, whatever Olo
is. For GNOME it takes its GTK 3 theme, which reads Olo's GTK theme
(the palette, light or dark), font and colour scheme and follows them
live, so Qt programs, Ladybird among them, change with Olo. It is the
one lever that reaches programs run in a toolbox or container too
(`toolbox run` passes `XDG_CURRENT_DESKTOP` but not
`QT_QPA_PLATFORMTHEME`); there the font is Olo's if the container has
it, else fontconfig's stand-in (Liberation Sans for Nimbus Sans). What
else reads the name: xdg-desktop-portal would read GNOME's portal
configuration too, so it alone is told the desktop is Olo (a runtime
drop-in, "Screen sharing"); `open.rs` reads
`gnome-mimeapps.list` after Olo's apps and before `mimeapps.list`, as
GNOME would; Chrome keeps its keys where it did (it treats GNOME and an
unknown desktop alike, libsecret, and asks the secret portal first
either way). A session started on an older build said only `Olo`, and
re-execs keep their environment, so `olo_xdg::upgrade_env` corrects it
when olo-session, the kit's apps (`Binary::exec`) and termd re-exec
after an upgrade, and a resumed real session exports it to systemd and
D-Bus activation again; programs already running keep what they had
until they restart. Test Qt programs in a headless session with a
throwaway HOME, `GSETTINGS_BACKEND=keyfile` and the colour scheme and
GTK theme in `$XDG_CONFIG_HOME/glib-2.0/settings/keyfile` (edit it to
switch live), and a bus that isn't there.

Window chrome: app_server offers server-side decorations two ways.
xdg-decoration (`zxdg_decoration_manager_v1`) is what Chrome, Qt and most
others ask through. GTK 3 and GTK 4 don't speak it; they speak KDE's
`org_kde_kwin_server_decoration`, whose default mode app_server
announces as Server, so a GTK window with no headerbar draws no
decorations of its own and gets Olo's tab and frame
(`KdeDecorationHandler` in state.rs, `kde_server_side`, read in
`shell/xdg.rs` with the xdg-decoration mode). A window that asks for
Client (a GTK headerbar, so every libadwaita app) keeps its own,
coloured by the theme. Test with a throwaway GTK script in a headless
session (`tsession run NAME GDK_BACKEND=wayland python3 test.py`, the
private bus as DBUS_SESSION_BUS_ADDRESS).

## Tracker (apps/tracker)

On Olo's GPUI. The `Tracker` entity (main.rs) keeps the windows (each
known by its view's entity id) and does what opens windows or touches
files: opening folders (a folder's window on this workspace is brought
forward, else one opens here: `olo_ui::workspaces`; File ▸ New Window,
⌘N, the `new` command, always opens another, on the folder it's pressed
in, Home from the desktop or for `--new-window`; New Folder is ⇧⌘N, as
Finder has them, both by `app_keys`' Tracker entries; Go ▸ Back and
Forward, ⌘[ and ⌘] on the Mac profile, Alt+← and → on Windows', and the
mouse's back and forward buttons go through the folders a window showed
in that window, as Haiku's Tracker navigates in one: `FolderWin::history`,
the folder it was opened from first (`came_from`, `open_from`), the one
it comes back out of selected), Get Info, Find,
the file operations (ops.rs, on workers; Tracker Status after 400 ms),
drops, pastes, Quick Look, the watcher's changes and an upgrade's saved
state (the egui Tracker's, unchanged). Each window's view does the
rest itself and hands the Tracker what it can't (`folder::Action`,
deferred so the Tracker can update the window back):

- `FolderWin` (folder.rs, model.rs, columns.rs): the folder's items
  listed on a worker and kept by the watcher, the selection kept by
  item, in Olo's order (`model::Order` None: the folders by name, then
  the files most recently changed first; the list's header shows
  Modified ▼) unless an order was chosen for that folder by hand (a
  column clicked, Window ▸ Sort By), which it keeps
  (`sorts.rs`: `~/.local/state/olo/tracker/sorts.json`, by folder, as
  BeOS kept a window's columns in its folder; a file of Tracker's own
  since the system's folders can't be written) and opens in again in
  any window; Sort By ▸ Folders by Name, Files by Date goes back to
  Olo's and forgets it; icon and mini-icon views drawn in a canvas from what shows
  only (items.rs: labels wrapped to two lines, four when selected,
  kept by text, font, width and scale so a scroll or a band shapes no
  name it has shaped before; the selected icon tinted and its label
  inverted), the band, the keys and
  typing to select; the list view the kit's `ColumnListView` (its
  `ListModel` follows the window's selection); the column view's
  columns side by side, scrolled across, with a preview of a selected
  file (thumbs.rs: pictures made small on a thread of their own).
  Editing a name is a `TextField` over the label (items.rs `Rename`);
  Return or a click elsewhere renames, Escape doesn't.
- `Desktop` (desktop.rs): a background layer anchored to every edge;
  icons placed by hand kept in `desktop.json`, the rest on the grid.
- Drops come through GPUI's drop API: each view takes what's dragged
  over it as it paints (`items::take_drop`, BeOS's rules: move within a
  file system, copy across, ⌥ copies, the secondary button asks;
  `items::Spring` opens a folder a drag rests on), and the window's
  drop handler gives the Tracker the drop. Drags out carry Olo's own
  types (the folder and file system they're from, whether to ask).
- Paste reads the clipboard off the UI thread (`App::paste_as`).
- `quicklook.rs`: olo-preview's `PreviewView` with the footer and keys;
  its window opens when the preview comes (or after 120 ms with a
  spinner) and has the keyboard.
- **Open With ▸** on a file's or folder's menu: the kit's
  (`olo_ui::open_with`, see "The GPUI kit"), for what's selected when it's
  all one kind (`folder::open_with_menu`, the kinds warmed as the
  folder is listed); on the desktop's items too, and on Quick Look's
  Open button (its secondary click). Its commands go through
  `Tracker::open_with`, which opens Other…'s window itself (to keep it
  in its windows' list) and hands the rest to `open_with::run`.
- `goto.rs`: Go ▸ Go to File or Folder… (⇧⌘G, Finder's Go to Folder),
  a dialog over the folder's window: the field holds a path copied
  (absolute, `~/…` as written, a `file://` URI's:
  `olo_kit_base::files::written_path`, which takes quotes and a
  trailing newline too, as the palette's Go to does), selected, so Return goes; the
  names its folder has that what's typed could be listed under it,
  each folder read once on a worker; Tab completes (the chosen name, or
  as far as they agree, case aside) where there's one to complete to,
  else moves on as Tab does (`capture_action` of `FocusNext`); ↑↓
  choose, and Return then puts the name in. Go reveals (`Tracker::
  go_to`: a folder opens, a file is selected in its folder's window); a
  relative path is from the folder it was asked in; nothing there, it
  says so and stays. Agents and the palette: `go-to` opens it,
  `go-to:<path>` goes straight there.

## Agent Screens (apps/screen)

Agents get screens of their own: `olo-screen start [NAME] [--size WxH]
[--scale N] [--sound]` starts a headless Olo session (`olo-session
--headless`, in its own process group), a whole desktop with its own
Deskbar, windows, pointer and keyboard focus, so nothing an agent does
there moves the person's pointer or takes their focus. It runs the
person's programs and files (their HOME), but with app_server settings
of its own (`OLO_APP_SERVER_SETTINGS`, a copy of the person's at 1×
unless asked, with no lock, screen saver, display sleep or system
sleep), silent unless `--sound` (`PULSE_SERVER` pointed at nothing),
without `DISPLAY` (X programs would show on the person's screen), and on
a D-Bus session bus of its own (below, "A screen's own bus"). Screens
are listed in `$XDG_RUNTIME_DIR/olo-screens/NAME.json` (the session's
pid, its sockets, the size); `screens.rs` checks the pid is still that
session before trusting an entry, by the `OLO_SCREEN=NAME` it was started
with (`is_session_of`: kept across an upgrade's re-execution; its command
line, which a session re-executed bare before 2026-11-07, isn't, so
every upgrade had `list` drop each running screen, and starting it again
by name made a second, ten left running unknown). An entry is dropped
only once its session has ended (`running`), `stop` ends the session's
whole process group if it hasn't ended 5 s after being asked (and keeps
the entry while it runs), and `start` and `stop` end any screen's
session nothing lists (`strays`: of that name whatever its age, others
once two minutes old so one being started isn't; never a session
inside a screen's). The rest is the screen's own app_server
IPC (`Client::connect_to`): `run` (a program with the screen's
`WAYLAND_DISPLAY`, `OLO_SOCKET` and `OLO_SCREEN`), `look` (a PNG,
`--window ID` for one), `windows` (`Reply::Windows`, whose `WindowInfo`s
carry `frame`, their place on screen, and `content`, where the app's
drawing is inside the tab and border; the `WindowsChanged` event leaves
them out, or a drag would send one every frame), `tree` (each kit app's
`commands::tree`, matched to its windows by title and size (Chrome's by
the page title its window's title starts with: see "Chrome's widget
tree") and moved by
`content`, so every node is in screen pixels, ready for `click`; an
outline, or `--json`; a program not on the kit, GTK 3 or 4, Qt, gives
its tree over AT-SPI instead, `atspi.rs`: its connections to the
accessibility bus found by its process id, each window's accessibles
walked while they show, roles named as AccessKit names them, a "push
button" a `Button`, states as the node's flags, a field's text and a
slider's number as its value, containers and a button's own label
left out, and places from the window's geometry, as GTK gives them;
each question has 1.5 s, so a hung program can't hold `tree` up.
olo-session sets `QT_ACCESSIBILITY=1`, as GTK is always there.
Tried with GNOME Calculator (GTK 4): its buttons clicked by name, its
display set by typing, `wait --text` on the answer. To try one
headless, the accessibility bus must be dbus-broker (registryd crashes
on busd): `dbus-broker-launch --scope user --config-file
/usr/share/defaults/at-spi2/accessibility.conf` given a listening
socket as fd 3 (`LISTEN_FDS=1`), `at-spi2-registryd` and the program
with `AT_SPI_BUS_ADDRESS` set to it, and a screen entry in a private
`XDG_RUNTIME_DIR`), `menus` and `invoke` (a window's app's menu bar
and choosing from it, `commands::menus`/`invoke`), `click`, `move`,
`down`/`up`, `drag`, `scroll`, `wheel`, `key` (chords as people write
them: `Cmd+S`, `Enter`, `Esc`; parsed by `olo_protocol::Chord` with
aliases) and `type` (what a US keyboard types as keys, anything else
— é, emoji, CJK — put in whole with `Request::InsertText`, in order),
all as `Request::Synthetic`, in the
screen's logical pixels. `stop` ends the session, and with it the
programs on it. `olo-screen --help` lists it all. Each action on a screen
(`history::ACTIONS`: `start`, `stop`, `run` and every input, find and
window command; not the reads) goes in its history,
`$XDG_STATE_HOME/olo/screens/NAME/actions.jsonl`, a JSON object a line
as each ends: when, how long, its arguments, its answer (what `print_json`
printed: what it matched, where it clicked, the focus after) and its
error, kept under 8 MB by dropping the oldest half and outliving the
screen until `history NAME --clear`. `history NAME` lists it (`--kind`,
`--search`, `--last N`, `--json`); `stats [NAME]` sums it
(`history::stats`: first start and last use, time running from each start
to its stop or now, time in use as the actions and the gaps under a
minute between them, actions by kind, failures, programs run) with
whether each screen runs now and its windows. Beside it are the frames
the screen drew (`record.rs`): `olo-screen record NAME`, started with
each screen in its process group, takes the screen's picture (as the
viewer does, RGBA) eight times a second for five seconds after each
action (the history's file changing) and once a second otherwise, and
keeps the 64-pixel tiles that changed since the frame before,
zstd-compressed, with a whole keyframe every 30 s, in five-minute
segments (`frames-START.olof`; the format is in the module's comment),
the oldest dropped past 512 MB. `record::index` reads the records'
heads, `frame_at` builds the screen as it was at a time from its
keyframe; `history NAME --look N [OUT.png]` writes it as it was when
action N ended. In the window, History (a tab beside Live and Stats;
`replay.rs`) lists its actions as they come: a worker follows the
history's folder (`olo_ui::watch`) and reads only what was added
(`history::Reader`, from where it last read to the end of its last
whole line; all again when the file was cut down or replaced) and the
frames' segments that changed (`record::index_known`; records still
being written wait), at most four times a second and the frames alone
once a second, with each action's search text made there. A search and
the kit's filter menus narrow the list (`kind:` Pointer, Keys and
Typing, Menus and Choices, Windows, Programs, Start and Stop, or a
command's own name; `result:failed|ok`; `since:5m|1h|today`), picked
within 2 ms a frame however long the history (`go_on_picking`), the
count of shown and all at the right. It shows the screen as it was at
the action chosen or wherever the scrubber is (in a sunken well,
`picture::PictureWell`), what the action was and what came of it under it
(what it matched, the program's pid, the window in front after, or why
it failed), and plays it back in time with them (Play/Pause, 1× to 8×,
◀ ▶ action by action among those shown; stretches of more than 3 s with
nothing in them skipped), each click a ring where it went and the
newest key or text a caption under the picture. Playing draws again
only when something shown changes (`Replay::play_on`: the next frame or
action, the scrubber's next device pixel, the time's next second;
every frame only while a ring grows or a caption fades), so a long
history plays at a few frames a second. At the newest action it
follows the screen (Live, or Latest for a stopped one, pushed while it
does): each new action is chosen as it comes and brought into view
once, so the list scrolls freely otherwise, and rings and captions fade
on the clock. Narrow, the scrubber goes over the buttons. Frames are
built on a worker, the latest asked for winning, played on from the
last (`record::Player`) rather than from a keyframe each time.
`history NAME --export OUT.mp4` (`export.rs`) makes it a video in
process: the frames at 10 a second (`record::Player`, each record
applied once in order), H.264 from OpenH264, in an MP4 of our own
(`ftyp`, `mdat`, `moov` with `avcC` from the encoder's parameter sets;
`co64`, one chunk). Stretches over 10 s with no action and no frame
changing are cut, each a two-second card in the kit's type ("4 min 12 s
with nothing happening", `ab_glyph`); `progress` gets how far it is and
can stop it. Olo's Viewer plays the result. In the window, History's
Export Video… asks where (the kit's Save panel, `NAME.mp4` in ~/Videos)
and exports on a worker, the bar saying how far it is with Cancel, then
an alert with Open in Tracker (`Replay::start_export`). The sidebar (the
kit's `Sidebar`) has every screen running and every one stopped, under
Running and Stopped, by last use (`viewer::rows`,
read on a worker; the chosen one's as it acts), each with how long ago,
and New Screen under them. A screen can be given a name of its own
(Screen ▸ Rename…, or a click on its title in the header: a field over
it, Return keeps it, Escape puts the old back; `olo-screen rename NAME
[TITLE]`): kept with its history (`history::title`, a `title` file
beside its actions, gone with it), shown in the list and the header,
and in `olo-screen list` as `title`. Screens are chosen together with
⌘-click (adding or taking one away), ⇧-click (the run from the last),
⌘A in the list or Edit ▸ Select All Screens; a secondary click opens
the menu for all of them (for the one clicked alone if it isn't one),
as the Screen menu has it too, each item saying how many it acts on:
Rename…, Start, Archive, Unarchive, Stop… and Delete… (asked once for
all). Every screen is kept from its start, not only once an agent has
acted on it: `screens::start` writes what it was started with
(`screen.json`: size, scale, sound) beside its history, which is what
lists a stopped one (`history::names`: a folder with `screen.json` or
`actions.jsonl`) and starts it again as it was (`screens::kept`; Start,
the Start Screen button in a stopped one's header, `olo-screen start
NAME`, which take it out of the archive too). Before, one made with New
Screen and stopped before an agent did anything on it went from the
list altogether. Archived screens (stopped ones, `history::archived`,
an `archived` mark beside the history) are listed under Archived
unless View ▸ Show Archived is turned off; Delete moves each screen's
folder (its history, recordings, title, Chrome profile and the
passwords shared with it) to the Trash (`history::trash`,
`olo_xdg::trash::move_to_trash`, with its .trashinfo to put it back).
The same for agents: `olo-screen list` gives each screen's `state`
(running, stopped, archived; a stopped one with its kept size),
`archive`, `unarchive` (`screens::archive`, a running one refused) and
`delete` (`screens::delete`, stopping it first). The chosen screen's
header names it and says how it is (running and its size, up for how
long, its actions, the last how long ago), with Export Video… and Stop
Screen… (for a stopped one Clear History…, which deletes its actions
and the frames kept with them and nothing else, and Start Screen). Stats (`stats.rs`) are the kit's dashboard
cards: its use (started, last used, ran for, in use), its actions by
kind with bars and how many failed, actions a minute over the last hour
of use, and the programs run on it (`history::stats`, worked out again
as the history grows or once a second while it runs). View ▸ Live,
History, Stats (⌘1–3), Find (⌘F), and Playback's Play, Previous and
Next Action and Go Live drive it from the menus.
How much is kept, and how long, is Preferences ▸ AI's Agent Screens
(`AiSettings::screen_history_mb`, `screen_history_days`; 512 MB a screen
for a month): `history::limits` asks the person's app_server, else the
screen's own, and each screen's recorder applies them as it starts
(`history::prune`: actions and frame segments older than that go, and
histories left empty; segments past the size, oldest first).

Input that plays out over time is paced, a batch per IPC request with
waits between (`Screen::input_paced`): `drag` presses, moves in steps
and lets go over ~300 ms (a knob or a window's corner needs to see the
movement); `scroll` moves in ≤20 px steps every 8 ms and rests 80 ms
before the fingers lift, so app_server doesn't glide on (a lift arriving
at speed is a flick) and the lift ends the gesture (a list pulled past
its end springs back); `wheel` sends one notch at a time. `find.rs`
finds elements by what they are (`--name`, exact before partial, values
counting; `--role`; `--match ROLE:TEXT`; `--text`; `--index`;
`--window`) for `click`, `set` (click in, checking it took the focus,
then ⌘A, Delete and the text), `select` (open the dropdown, wait for the
`MenuItem`, click it), `wait` (until it shows or `--gone`; `--idle`: two
outlines a quarter-second apart the same, and no window settling) and `scroll-to` (wheel to the
top, then down, until it shows whole); actions print JSON with `after`,
the focused window and element. Popups are in the trees: the kit paints
its menus (no AccessKit), so `Kit::popup_nodes` adds each open popup's
`menu::popup_nodes` (a `Menu`, a `MenuItem` per item in view, laid out by
the same `PopupLayout` that draws them) to its owner window's tree,
placed by the popups' configured positions. `WindowInfo::popups` gives
every window's open popups' frames, GTK's too (smithay's
`PopupManager::popups_for_surface`, from the client's geometry). Table
rows carry their place (`UiNode::row`/`rows`, printed `row 13/61`)
and whether they're `selected`, as tabs and column headers do. `focus`/`close`/`minimize`/`zoom` are the window
requests; `resize` and `place` are `Request::Place` (a frame, as
`set_frame` gives tiles). `look` writes beside the logs when given no
file, and `--crop` cuts an RGBA picture down in process (png crate).

The person's side is the window (`olo-screen`, or `olo-screen
watch NAME`; on the GPUI kit): the screens in a sidebar and the chosen
one's Live tab, the screen with the agent's pointer drawn in and its
latest action under it. Live shows the screen's own frames as it draws
them, never copied (`viewer::stream_live`, `olo_capture::live`):
- The screen's app_server captures each frame on the GPU into one of
  three dmabufs in turn. Frames come only when the screen changed, at
  its refresh (agent screens start at 120 Hz, `screens::fastest`).
- The viewer hands each buffer to its window (`Window::set_external_buffer`,
  `ExternalBuffer`), and the person's app_server shows it in the well's
  place as a subsurface (`Window::place_external`, `picture.rs`).
- A buffer is captured into again once the person's app_server lets it
  go (`wl_buffer.release` → `Waker::released`).

The window draws nothing for a frame, and nothing is read back. Dragging
a window on the screen for 5 s: the viewer used 0.6% of a core for
~60 frames a second (120 at 120 Hz). Picture by picture it used 30% for
8 a second and missed pointer-only changes.

The picture's corners sit at whole points (subsurfaces can't be placed
finer). `Window::external_bounds` gives the device pixels the compositor
rounds them to, so the frame line drawn round it is flush at every
scale; measured at 1, 1.25, 1.5, 1.75 and 2×, resized to odd sizes.

Where the two app_servers share no dmabuf layout (`App::external_modifiers`
empty, or `OLO_SCREEN_PICTURES` set to compare), it takes pictures as
before:
- one worker follows the screen's `EventKind::Painted`; another asks for
  a `format: "rgba"` picture then, at most eight a second, and reads it
  back (an app_server from before `Painted`: eight a second);
- each picture is resampled on a worker to the device size it's shown at
  (the last stands in until it's made).

Either way only while Live shows, so a screen at rest costs nothing to
watch. A live view that ends (the screen's app_server restarting) starts
again every half second. Take Control (⌘K) steps in: the person's pointer,
wheel and keys over the picture go to that screen through another worker
over a connection kept to it, moves run together to the last (keys by
their evdev codes on a US keyboard, the agent's, from the keys GPUI
names; Ctrl, Shift and Alt pressed and let go as each key event says, a
held key repeated by the screen, Tab and ⌃Tab to the screen too, ⌘K
still stopping), with a ring in the keyboard-navigation colour round
the picture, until Stop Controlling (everything held let go). New Screen starts one; Stop
Screen… ends one after asking. A viewer running on an agent screen
leaves that screen out of its list. While any screen runs (other than
the one a Deskbar is on), Deskbar's tray shows a monitor with a pointer,
and their count when there are several (`status::agent_screens` reads
the same listing as a screen's file comes or goes, `olo_ui::watch`, and
every 30 s while there are any for one killed without a word, judging
entries as `Screen::alive` does; a viewer's frames go in `views/` beside
them, so they don't wake whoever follows the listing);
clicking it runs `olo-screen watch`. Test it in a sandbox: a private
`XDG_RUNTIME_DIR`, a fixture HOME and XDG dirs (the screen runs the real
programs), and a second screen to run the viewer on.

Agents learn all this from the **`olo-screens` skill**
(`apps/screen/SKILL.md`): a real session (`--tty-udev`, at start and
after each upgrade; `crates/session/src/skills.rs`, on a thread) puts it
in `~/.claude/skills/` (or `$CLAUDE_CONFIG_DIR/skills/`) when Claude Code
is there and in `~/.agents/skills/` when Codex is, replacing only a copy
that still has Olo's marker line. **Change SKILL.md with the CLI**:
it is the agents' manual. Headless sessions never write it, so tests
don't touch the person's home.

While the person controls a screen (Take Control), the viewer writes
`NAME.control` (its pid) in the screens' folder, and `olo-screen`
refuses every action on that screen (`history::ACTIONS` but `start`,
and `maximize`) with "the person is controlling screen NAME …: nothing
was done. Wait until they hand it back (olo-screen wait NAME --free)",
kept in the screen's history like any action; reading it still answers.
Stop Controlling, a quit or another screen chosen removes it; a viewer
that has gone (its pid not an `olo-screen`) controls nothing, and one
re-executed by an upgrade forgets its own (`clear_controlled_by_me`).
Two hands on one pointer made a mess before (Ben: "all kinds of weird
stuff happens").

While the person controls a screen, its clipboard and theirs are one
(`clip.rs`): what they copy on the screen is on their clipboard and what
they copied on their own display pastes on the screen, every form of
it (text, pictures, files), both ways, each side's clipboard watched
(`clipboard::watch`) and what came from the other side not sent back
(by a digest of it). Handed back, what of theirs is on the screen's
clipboard is taken off it (an agent's screen never has the person's
clipboard on its own); what they copied from the screen stays on
theirs. It's joined off the UI thread (two displays' connections).

### Moving windows to and from screens (apps/screen/src/moving.rs)

The person can put any of their windows on an agent screen (for an agent
to work in it: a Terminal tab, a Chrome window, a document), and give a
screen's windows to themselves; the program keeps running as it goes.
`olo-screen take NAME|--new --window ID…` moves the person's windows
(ids on their display) onto a screen, and `olo-screen give NAME
[--window ID…] [--workspace N] [--display OUTPUT]` the screen's (all of
them by default) to the person's display, onto one of its workspaces;
both answer JSON (`went`: each program, how it went and its windows'
new ids; `stayed`: what didn't, and why), go in the screen's history,
and are allowed while the person controls the screen (they're the
person's moves). `moving::move_windows` groups the windows by program:
- An app on Olo's kit is asked over its command socket (`{"move":…}`,
  `olo_kit_base::moving::MoveTo`: the window's surface, or none for the
  whole app, and the environment of the display it goes to). Its only
  window, or all of them, and it re-executes there with its state
  (`on_upgrade`'s), as for an upgrade, keeping its pid
  (`upgrade::move_to`). One window of several, or an app that is also
  part of the desktop (a layer: Tracker's desktop; resident), goes by a
  copy of the app started there with that window's state
  (`on_move_window`, `moving::start_copy`) and let go here: Tracker's
  folder and Find windows, Terminal's windows. Otherwise it says why it
  stays.
- Any other program is moved by the olo-wlproxy holding its connection
  (`move PID PATH`, above). A program's connection is all its windows,
  so they all go (`went` says so).

A moved kit app is started with `OLO_MOVED` (claimed as it starts), so it
never hands itself to a copy already running there (`instance::forward`
is skipped, and `instance::listen` leaves that copy its socket). The
person's app on a screen carries `OLO_HOME` (`moving::env_for`: what its
environment was, a `MoveTo` back); dropped as it goes back. It goes back
when the screen stops (`screens::stop` first runs `moving::send_home`:
each proxy's `home`, then each kit app with an `OLO_HOME` asked to move
back, and a moment for them to go) or ends without a word (a kit app
whose compositor is lost while its screen's session or socket is gone,
`upgrade::going_home`, waits up to 2 s to tell an end from a restart; the
proxy's `AWAY_GONE`). A program started on a screen and given to the
person stays theirs when the screen stops (a kit app re-executed with
their environment; another program served on by the screen's forked
proxy). Terminal: its shells live in the olo-termd of the display it
started on, which a moved Terminal keeps talking to (`wire::display`
reads `OLO_HOME`), and a window or tab goes by a Terminal on the screen
attaching its shells (`Saved::moved`: taken from the other Terminal,
whose tab goes as olo-termd detaches it, never hung up; no other shells
opened); a Terminal started on a screen stays there
(`refuse_moves_while`: its shells end with the screen).

Where it's offered (`olo_kit_base::moving::screens()` lists the screens
running, by the name the person gave each; only on the person's own
display, `menus_offer`, and only for windows whose program can move,
`WindowInfo::kept`; each runs `olo-screen take` on a thread, a
notification saying why if nothing moved, `take_to_screen`):
- the window menu (a window tab's right-click): Move to Agent Screen ▸
  each screen, New Agent Screen (`Menu::agent_screens`);
- Deskbar's app menu: Move (All) to Agent Screen ▸; a workspace
  heading's menu: Move All Windows to Agent Screen ▸;
- Terminal's tab menu: Move to Agent Screen ▸ (the tab alone, by a
  Terminal on the screen: `moving::copy_to_screen`);
- Agent Screens, beside Take Control: Move Windows To… (and Control ▸
  Move Windows To ▸): every window on the screen to one of the person's
  display-workspaces (`moving::give`, on a worker; an alert says what
  stayed).

Tested in a private world (its own `XDG_RUNTIME_DIR`, HOME and bus, a
stand-in person's `olo-session --headless` and a screen beside it):
DeskCalc and GNOME Calculator there and back (the calculator took input
on the screen), each coming home when the screen stopped and when its
session was killed without a word; a screen's DeskCalc and zenity given
to the person outliving the screen (its stop 0.05 s with the forked
proxy, 3 s before); a Tracker folder window and a Terminal window and
tab moved by copies, the Terminal's shell the same process on the
screen (`echo $$`), all of them back when the screen stopped.

### A screen's own bus (crates/bus, crates/session/src/own_bus.rs)

On the person's session bus, a program started on a screen found the
person's copy of itself by its name (GTK's GApplication) and had it open
the window on their display, a portal dialog it asked for (Chrome's
upload, Open With) showed on their display, and its notifications popped
up there. So each screen's session runs a bus of its own: `olo-screen
start` passes `olo-session --own-bus unix:path=$XDG_RUNTIME_DIR/
olo-screens/NAME.bus`, and the session starts `olo-bus` there before
anything else (waiting for its "ready"), points its environment at it
(`DBUS_SESSION_BUS_ADDRESS`), and looks after it with the shell (started
again if it ends; ended with the session, and with `--with-parent`
however the session ends). The listing has it (`Screen::bus`), so `run`
and moves give programs the screen's bus (`moving::bus_env`), and a
move back gives the person's back. A screen from before has none and
stays on the person's.

What the person's session has on their bus, the screen has on its own
(`own_bus::services`, with the shell): its own notification centre
(`olo-notifications --bus`, shown on the screen: agents see their
programs' notifications, the person doesn't get them), Olo's portal
backend (`olo-portal --bus`) and GTK's (Settings: the colour scheme),
behind xdg-desktop-portal, so Open and Save panels and the rest show on
the screen. Nothing on the screen's bus starts on demand, so `olo-bus`
stands in for what the frontend needs (a permission store in memory, so
the person's on-disk one is never written by two; the document portal's
mount point, the person's: sandboxed programs' file sharing isn't
offered on a screen) and waits for the backends the session is starting
when they're asked for (`--expect`). The keyring isn't there: the
person's secrets reach a screen only through Passwords' grants
("Passwords for agent screens"). The accessibility bus stays the
person's (`olo-screen` asks theirs for its address: `OLO_A11Y_BUS` to
the session, `AT_SPI_BUS_ADDRESS` to programs, and `org.a11y.Bus` on
the screen's bus answers it), so `tree` reads GTK and Qt programs there
as before.

`olo-bus` is busd 0.5.0's broker (MIT; `crates/bus/LICENSE-MIT`) on
async-io and smol rather than tokio (zbus's `tokio` feature would have
turned every Olo program's zbus to tokio: Cargo unifies features), Unix
sockets only, lenient about match rules as dbus-daemon and dbus-broker
are (spaces, `eavesdrop`: Chrome's, `fdo::dbus::lenient_rule`), and it
answers a call to a name nobody has at once (`ServiceUnknown`;
`Peers::answer_unknown`), where busd left the caller waiting out its own
timeout (GLib's 25 s: GTK programs on test buses took that long to show).
tsession's sessions use it too, when built.

An Electron app (Slack, VS Code, Discord) finds its running copy through
a lock in its config folder, not the bus, so one started on a screen
gets a config folder of the screen's own (`XDG_CONFIG_HOME`, beside its
history as Chrome's profile is: `olo_xdg::electron`, known by its
`resources/app.asar` or a launcher script naming one), from `olo-screen
run` and from anything a screen's own programs start (`olo_xdg`'s
`spawn`, `spawn_app`, `spawn_detached`, which see `OLO_SCREEN`): signed
out, its own. Tested with Slack running for a stand-in person and
started on the screen: two Slacks, each on its display.

Tested in a private world and on a real screen: the portal's Open panel
asked on a screen shows there; GNOME Calculator runs on the person's
display and on the screen at once (before, the screen's handed itself
over); a notification sent on the screen is the screen's centre's; Olo's
Chrome opens in half a second with no match rule refused; `tree` reads
GNOME Calculator's buttons on a screen with its own bus; the bus ends
with the screen.

### Passwords for agent screens (apps/screen/src/shares.rs)

Agents on a screen get the passwords the person chose, and only those:
items shared with every screen (Screen ▸ Passwords for Every Screen…)
and, in each screen's Passwords tab, more or fewer for that screen. Both
lists are every item Passwords has (titles, usernames, vaults, kinds: no
secrets), searched and filtered in the kit's language (`vault:`,
`kind:`, `shared:yes`), several chosen at once (⇧, ⌘, ⌘A) and shared
or not with Share and Stop Sharing. The tab says how each stands: Every
screen, This screen (besides), Not here (every screen's, taken away
here).

The choice is kept in the person's store, sealed with it
(`vault::Shares`: `everywhere`, and each screen's `ScreenShares { add,
remove }`), and asked and changed over the keychain socket
(`Request::Shares`, `SetShares`), which waits for Passwords to be
unlocked and refuses a program on an agent screen (`OLO_SCREEN` in its
environment, `shared::on_a_screen`), so an agent can't share itself
more. After anything that changes the store, Passwords writes what each
screen has as a store of its own (`Vault::write_copies`): every
screen's in `~/.local/state/olo/screens/passwords.vault`, and one in
`screens/NAME/` for each screen given more or less (gone again when it's
back to every screen's), each sealed with a key kept beside it (0600),
an unchanged copy not written again. This is the person's choice
(2026-11-03) over sharing only while their Passwords is unlocked: a
screen's items are there whether or not their Passwords is open or
unlocked, and are readable by anything that can read the screen's key.

Passwords started on a screen (`OLO_SCREEN` set) opens its copy, or
every screen's, with that key alone (`Vault::open_shared`): no lock, no
setup, no syncing, read only (no New Item or Edit; the store refuses
changes), the person's settings left alone, `op` answered without asking
(what's there was given to the screen's agents), and the copy followed:
shared more or less, it opens again (`SharedChanged`). Its status line
says "N items shared with this screen". To try it: `cargo test -p
olo-passwords fixture -- --ignored` writes a store with a few items
(`OLO_FIXTURE_VAULT`, `OLO_FIXTURE_KEY`).

## Google Chrome (apps/chrome)

`olo-chrome` is Google Chrome in Olo's applications (`OUR_APPS`).
With Chrome installed it execs Chrome's own launcher
(`~/.local/share/olo/chrome/current/google-chrome`) with its arguments,
adding `--ozone-platform=wayland` under Wayland (Chrome otherwise goes by
`XDG_SESSION_TYPE` and falls back to X11). Each profile it starts gets
Olo's defaults once (`prefs.rs`: Olo's title bar, the GTK theme, no
password saving of Chrome's own) and Passwords' extension, the first
start after installing too. On an agent screen (`OLO_SCREEN`) a profile
starts ready for an agent: Chrome's `First Run` mark written in its
folder, so no terms of service window, first-run or welcome pages (a new
screen's profile showed Google's terms, which agents couldn't get
past), `session.restore_on_startup` set to the New Tab page (no
"Continue where you left off" bar) and `--no-default-browser-check`;
the person's own first Chrome still shows them Google's terms. Without it, it offers to
download it in a window on the GPUI kit (an `AlertFrame` with the
installer icon: the offer, the download's `StatusBar` with Cancel, or
why it failed with Try Again; Return and Escape as an alert's; 54 MB
against egui's 77) (`install.rs`, on a worker thread, cancellable): Google's rpm
repository for this CPU (`repomd.xml` → the primary index → the newest
`google-chrome-stable`), the package downloaded with live progress and
its sha256 checked against the index, then unpacked natively (the rpm
lead and headers, the payload's compressor from tag 1125, a "newc" cpio
stream; only `./opt/google/chrome/`, no setuid bits, no `..`) into
`<version>/`, with `current` switched over by a rename and older versions
removed. It installs Chrome's icons into the hicolor theme and a
`google-chrome.desktop` that starts it through `olo-chrome` (NoDisplay,
as Olo lists it itself; rewritten when it isn't ours). Nothing needs
root. Once a day (`.checked`), a start also runs `olo-chrome
--olo-update` in the background: a newer version is put in beside the
running one and becomes `current` for the next start (the one it
replaced is kept, as Chrome reads its files as it runs; older ones go;
`.updating` keeps it to one at a time). Test it with a fixture HOME (`tsession run NAME HOME=… XDG_DATA_HOME=…
target/debug/olo-chrome`), never Ben's profile.

With a Chrome already running on the profile folder (its
`SingletonLock`, `host-pid`, naming a live process on this computer),
`olo-chrome` hands it the command line itself (`singleton.rs`) instead
of starting a second Chrome only to do that: Chromium's process
singleton protocol, `START`, the current folder and the arguments (the
program first), each after a NUL, written to `SingletonSocket` in the
profile folder, the write side closed, answered `ACK` (or `SHUTDOWN`: that
Chrome is going, so start one). The activation token goes along as
`--xdg-activation-token=…`, as Chrome's own hand-over passes it, so the
window comes forward. A link from `xdg-open` (a CLI's sign-in) so opens
in a millisecond and says nothing on the terminal, where a second Chrome
printed "Opening in existing browser session." and its zygote's
"Failed sending zygote boot message: Broken pipe" as it went. No Chrome
running, or no answer: Chrome is started as before.

**Passwords in Chrome** (`extension/`, `src/extension.rs`, `src/host.rs`).
Each start (before exec) packs `apps/chrome/extension` (built into the
binary, plus a `palette.css` from Olo's palette, light and dark) into
a CRX3 signed with a P-256 key of this user's own
(`~/.local/share/olo/chrome/extension-key.der`, 0600; the extension id
comes from it), and writes `<chrome>/extensions/<id>.json` naming it:
Chrome on Linux installs such an extension at start without asking, and
takes a higher `version` as an update, so **raise `version` in
`manifest.json` whenever the extension changes**. The listing is the
install's, so one extension serves every profile and user data folder
that Chrome runs (each gets it once): only an olo-chrome whose own
`~/.local/share/olo/chrome` holds the Chrome it runs writes it, and it
takes out any other Olo Passwords listing (a `*.json` naming an
`olo-passwords.crx` or `tabula-passwords.crx` signed with another key)
from every version folder, which Chrome uninstalls as it next starts.
One with a home of its own running another home's Chrome (a test's,
through a `current` link) lists nothing there and registers its host for
the install's extension, read from that install's key: listing its own
copy put a second Olo Passwords in each of the person's profiles. The worker reloads on
`onUpdateAvailable` (Chrome would otherwise wait for it to go idle,
which the open native port prevents) and puts `content.js` into the tabs
already open on install or update. Beside the profile goes
`NativeMessagingHosts/com.olodesktop.passwords.json`, naming `olo-chrome`
itself: started as `olo-chrome chrome-extension://ID/`, it is the
native messaging host (length-prefixed JSON on stdin/stdout), passing
only `logins`, `fill`, `check`, `save`, `unlock`, `show` and the
passkey requests on to Passwords over the keychain socket (`{id,
request, start}`; `start` starts Passwords and waits out its unlock;
`{cancel: id}` ends a waiting request's connection, which Passwords
sees as `Incoming::gone`) and `{open: true}`. The
extension: a field's context menu has "Autofill with Passwords (the best
login)" and Passwords ▸ each login ▸ Autofill, Username, Password,
One-Time Code (when it has one), and Open Passwords (Unlock Passwords…
while locked); Chrome puts an extension's items under its name when it
has more than one, so they show under Olo ▸. The best login is, in
order: one whose username the page shows (`seen` in content.js: its
text and fields' values, an account chooser, "signed in as"), the one
used on this site (its registrable domain, roughly) before in this
Chrome profile (`chosen` in `storage.local`, which is the profile's:
set by an autofill, or by a login signed in with by hand, found by its
username, a hidden username field or the one account the page shows),
the profile's own account (who is signed in to Chrome there), then
Passwords' order; the popup lists them so too. Which profile: the
extension keeps a random token in its storage, and the host finds the
profile whose `Local Extension Settings/<id>` holds it (`profile.rs`;
among those whose storage Chrome, its parent, has open, since Chrome
rewrites its command line into one string, else every profile of the
user data folder) and reads who is signed in from `Local State`. The
menu is rebuilt when a field gets the focus or the pointer comes over
one (Chrome shows it as it is when the button goes down). Fields are
found in open shadow roots too; the password is the focused one, else
`current-password`, never a `new-password`; the username the text field
before it (one that says it's for the account first); one-time codes by
`one-time-code` or what the field says (never a card's "security
code"), split over one-digit boxes when the site does; each filled as
typing does (keys, `input`, `change`). A sign-in in steps (the account,
then the password, then a code) gets the rest as it comes: after an
autofill that didn't fill everything, the tab is watched (a
`MutationObserver`, and the next page on the same site, for 3 minutes)
and a password or code field that turns up is filled from the same
login. A fill goes only to the frame it was asked from and only while
it is still at the origin the login was asked for. The
toolbar button lists the page's logins (Enter or a click fills one). A
password sent with a form (submit, a button, Enter) is checked, and a
new or changed one offered in a notification (Save/Update, Not Now; the
pending login is kept in `storage.session` only). Passwords names the
waiting app from the keychain caller's program (`Incoming::from`). Test
with a throwaway `--user-data-dir` (DevTools needs a non-default one:
`--remote-debugging-port`), the fake 1Password and a localhost page; the
fake's "Olo Test Site" login (`http://localhost`, jane) has a TOTP.

**Credit cards** (keychain `Cards` and `FillCard`; `card_fields` in
Passwords' `store.rs`; `cardKind`, `fillCard` in `content.js`). A card
form's field (by its `autocomplete` token, `cc-number`, `cc-name`,
`cc-given-name`, `cc-family-name`, `cc-exp`, `cc-exp-month`,
`cc-exp-year`, `cc-csc`, else by what it says: "card number", "CVC",
"expiry month", "name on card"…) tells the menu so as it gets the focus
or the pointer: "Autofill Card (Travel Visa •••• 1111)" on top, and under
Passwords ▸ every card ("Travel Visa (•••• 1111, 08/2029)"), then the
logins as ever. `Cards` gives what may be said of each card (title,
name, brand, last four digits, expiry; never the number or the code),
doesn't wait, and is kept like a page's logins; it is every active
Credit Card item, as a card isn't kept for a site. `FillCard` waits
for an unlock and answers only for a card (the candidates are the one
chosen, if it is one): the number (digits), name, month, year and code,
from 1Password's built-in fields (`ccnum`, `cardholder`, `type`,
`expiry`, `cvv`), else by their types and labels; wiped from the
response once sent. So any page can be given a card, but only by the
person choosing it in Chrome's menu, which a page can't trigger. The fill
goes to the frame the menu was opened in and to the tab's frames that
are the same site's or a payment provider's (Stripe, Braintree, Adyen,
Checkout.com, Square, PayPal and others, whose fields shops put in
frames of their own: `PAYMENT_HOSTS`), never another site's; each fills
the card fields of the form the person was in (else its own), typed as
a person would, selects chosen by value or text ("8", "08", "Aug",
"August"; "29" or "2029"), an expiry field in its own shape (its
placeholder's "MM / YY" or "MM/YYYY", else 4 or 6 long, else "MM/YY"),
a name split where the form asks for given and family names; the frame
the menu was opened in is filled last, so its field keeps the focus.
Tested end to end in a test session: the fake 1Password's Travel Visa
into a checkout page (one form by `autocomplete`, one by its labels with
month and year selects, a same-site frame and another site's frame),
through DevTools (`Runtime.evaluate` in the extension's worker: `cards`,
`buildMenu`, `fillCard`) and through the real menu: only the form the
person was in filled, the same-site frame too, the other site's never.

**Passkeys** (`extension/passkeys.js`, `relay.js`; Passwords'
`passkey.rs`; the keychain's `Passkeys`, `PasskeyCreate`, `PasskeySign`).
Passwords is the page's passkey authenticator in Chrome. `passkeys.js`
runs in the page's own world (`"world": "MAIN"`, `document_start`, every
frame) and answers `navigator.credentials.create`/`get` for
`publicKey` requests (an iframe only where its permissions policy lets
it); `PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable`
and `isConditionalMediationAvailable` say yes, so sites offer passkeys.
Its credentials are objects on `PublicKeyCredential.prototype` (and the
response types') with their values as own properties, so `instanceof`,
`toJSON()` and `getPublicKey()` work as on Chrome's. It talks through
`relay.js` (the isolated world, `postMessage` on its own window) to the
worker, which builds the client data from the frame's origin as Chrome
reports it (`sender.origin`, never the page's word), hashes it, and asks
Passwords, which checks the relying party fits the page
(`olo_keychain::rp_id_fits`: the page's host or above it, a secure
page, never a public suffix or an address). Making one: Passwords asks
in an alert in front ("Save a passkey for “ann@…” on example.com in
Passwords?", naming the login it joins), Use Another Device… / Cancel /
Save Passkey; it joins that account's login, else gets a login of its
own, in a "Passkey" section (P-256 key, attestation "none", the
backup flags set since 1Password syncs it, sign count 0); the item view
shows only its site and user. Signing in (`get`): when Passwords has
none the page may use, Chrome's own way at once (its sheet: a phone's
passkey by QR code, a security key); else an alert, with a radio
button per account when there are several, and Sign In signs. "Use
Another Device…" hands over to Chrome's own sheet; Cancel gives the
page `NotAllowedError`; the page giving up (an `AbortSignal`, leaving)
takes the alert down. Locked, Passwords answers from the index it keeps
while locked: none here means Chrome's way without an unlock; else it
comes forward to be unlocked ("Google Chrome is waiting for a passkey")
and then asks. A sign-in that suggests passkeys in its account field
(`mediation: "conditional"`) gets Chrome's own suggestions and, in the
field's context menu, Olo ▸ "Sign In with Passkey (who)" (the others
under Passwords ▸), which signs at once (the click is the consent);
whichever is chosen first wins and the other is let go. Passwords
filters items with a passkey (`has:passkey`). Test with a localhost
page that checks what a site would: the client data, the
authenticator data's site hash and flags, the attestation, and the
signature with WebCrypto.

**Downloads** (`background.js`'s Downloads, `downloads.rs`). The
extension (with the `downloads` permission: Chrome only asks again for
a permission an extension it installed itself gains, never for one in
its install's `extensions` folder) tells the host of Chrome's downloads:
`{downloads: [...]}` with those under way (id, file, received, total,
paused, url), on each start or stop and every half second while there
are any (Chrome says nothing as one goes), and `{downloaded}` when one
finishes or fails (with Chrome's `InterruptReason`). The host sends
Deskbar a `com.canonical.Unity.LauncherEntry` update for
`application://google-chrome.desktop` (the count, and the progress of
them all once each knows its size: Chrome itself would only through
libunity), marks the file being written (`NAME.crdownload`) with the
size it will be (`olo_ui::files::set_expected_size`, the
`user.olo.expected_size` attribute), and shows an Olo notification for
a finished one (the picture, if it is one; a click or Open opens it,
Open in Tracker shows it; one that failed for a reason other than being
cancelled says why). Tracker shows a `NAME.crdownload` as `NAME`, of the
kind it will be, with a bar under its icon (icon views, the column
preview) and "3.1 of 10 MB" / "Downloading, 31%" in its Size and Kind
(`Entry::downloading`, `label`, `size_text`, `kind_text`); Chrome renames
it when done, and the bar goes.

**Chrome's widget tree, for agents** (`extension/tree.js`, `host.rs`).
The extension (named "Olo": it does more than passwords) connects to
its host when its worker starts and stays connected (reconnecting after
10 s), so a host runs beside every Chrome. The host answers a kit app's
`tree` socket for its parent, Chrome's browser process (the Wayland
client app_server names in `WindowInfo::pid`), with
`commands::serve_trees`: each question goes to the extension as
`{tree: n}`, which injects `tree.js` into every frame of each window's
active tab and answers `{tree: n, windows}` within `TREE_WAIT`.
**Iframes**: in each frame, `tree.js` tells every iframe element a token
of its own (kept per element in the extension's isolated world, posted
to the iframe's window each time, since it may have navigated); the
frame inside keeps it (`content.js` listens; a frame not told yet waits
up to 150 ms) and reports it with its nodes. An iframe's node carries
its token and content box (its rect less border and padding), and
`stitch` (background.js) puts the frame's nodes under it, offset to that
box and clipped to it (and to every iframe around it), cross-origin
frames and nested ones alike, so `click`, `set` and `wait` work inside
embedded sign-ins, payment forms and consent dialogs as on the page. `tree.js`
walks the page (shadow roots too, at most 4000 nodes) as assistive
technology sees it: ARIA roles, else the element's (Link, Button,
TextInput, PasswordInput, CheckBox, ComboBox, Heading, Image, List, Row,
Cell, Dialog, …), "Clickable" for script-made buttons (an `onclick`, a
`tabindex`, or where the pointer cursor starts), and "Label" for text
not inside anything named; names from `aria-labelledby`, `aria-label`,
the control's `<label>` (which is then left out), `alt`, `placeholder`
or the text; values, checked, selected, disabled and the focus.
Passwords' values are never read (`••••••••`). Places are the viewport's
CSS pixels times the tab's zoom, the page set at the bottom of the
window below Chrome's tab strip and toolbar (`window.height − page
height`); a `Window` node and a `Document` node (title, URL) hold it.
Agent Screens matches a tree to its window by the page title the
window's title starts with. Chrome's own tab strip and toolbar aren't in
the tree. Test on an agent screen with a private `XDG_RUNTIME_DIR`, a
copy of Chrome in a fixture `XDG_DATA_HOME` (`olo-chrome` writes into
the install's `extensions` folder) with a fresh `.checked`, a throwaway
profile and a page served from 127.0.0.1.

## Slack (apps/slack, olo-slack)

`olo-slack` is Slack, Olo-fied, as `olo-chrome` is Chrome: Slack's web
app in Olo's Chrome, as an app window of its own (`--app`), on a Chrome
profile of its own (`$CHROME_CONFIG_HOME`, else the config folder, then
`olo-slack`: an agent screen has its own), so Slack's sign-in and data
are Slack's alone and the person's Chrome is untouched. It is one of
`OUR_APPS` ("Slack", Olo's `slack` icon). Chrome names an `--app`
window's Wayland app id after the URL and the profile folder
(`chrome-app.slack.com__client-Default`); app_server reports it as
`olo.slack` (`olo_xdg::apps::WEB_APPS`, `WindowElement::app_id`), so
Deskbar, the palette, Twitcher and agents know it as Olo's Slack. It
always starts on the same page (the app id comes from it) and goes to a
link once in: `olo-slack LINK` (a `slack://channel?team=T&id=C`, `open`,
`user` link, or a Slack page) leaves it in the profile folder (`Olo Slack
Link`) for the extension's host to take once. With Slack open,
`olo-slack [LINK]` brings its window forward (app_server's `Activate`)
and hands the link to it (the kit's commands socket: `open URL`), never
a second window. Chrome starts with `--no-first-run
--no-default-browser-check`: the first-run dialog's "Make Google Chrome
the default browser", ticked, would have made Slack's Chrome the
person's browser. `slack://` links open in it (`open::ours_for`).

The joining is the Olo extension's (apps/chrome: `slack-page.js`,
`slack.js`, background.js's "olo-slack" part) and its host's (`slack.rs`),
only in olo-slack's Chrome: olo-slack gives Chrome `OLO_SLACK_URL` and
`OLO_SLACK_PROFILE`, which reach the host as Chrome starts it; the host
takes them out of its environment at once (nothing it starts, a link's
browser, inherits them) and tells the extension `{slack: {url, open}}`.
The extension then registers its two Slack scripts for Slack's address
(`chrome.scripting.registerContentScripts`, kept by Chrome between
starts; the first time, Slack's page is reloaded so they are in from its
start):

* **Notifications** (`slack-page.js`, in the page's own world before its
  scripts): the page's `Notification` (and a service worker
  registration's `showNotification`, and `permissions.query`) is
  answered there, always granted; each goes to the host, which shows it
  as an Olo notification of Slack's (`Notifier`, "Slack", `olo-slack`:
  Message style, Slack's aubergine edge, the sender's face as its icon,
  fetched by the extension with Slack's sign-in and kept in
  `~/.cache/olo/slack/faces`; silent, as Slack plays its own sound; a
  tag's later one updates it in place). Olo's Notifications keeps its
  history and its Do Not Disturb. A click on one brings Slack forward
  and is the page's `click` (Slack goes to the conversation); its going
  is the page's `close`.
* **Unread count** on Slack's Deskbar row: the page's app badge
  (`navigator.setAppBadge`, answered in the page) once Slack sets one,
  else what its title says (`(3) …`: 3; `! …`: at least one), as a
  `com.canonical.Unity.LauncherEntry` count for `olo-slack.desktop`,
  sent only when it changes (and again to a new host). Slack's Chrome's
  downloads show only their progress there (`downloads::SLACK`): the
  count is Slack's.
* **Links**: a link out of Slack that would open a tab (a click on a
  `target=_blank` link, a middle click, Ctrl and a click: `slack.js`; or
  `window.open`: background.js's `tabs.onCreated`, the tab taken back)
  opens with Olo's default app for it, the person's own browser, not
  Slack's Chrome. Slack's own pages (its sign-in, `*.slack.com`) and a
  sign-in's popup stay.
* **Pictures**: a picture's context menu has Open in Viewer: the
  extension fetches it with Slack's sign-in and the host writes it to
  `~/.cache/olo/slack/pictures` and opens it in Olo's Viewer. The cache
  keeps faces and pictures 30 days.
* **The palette**: the host answers the kit's commands socket for
  Chrome's process (`commands::serve`, `Served`: menus and invoke beside
  the trees) with Slack's commands, asked of the page as the palette
  asks (nothing is watched between): Jump To… and Search… (Slack's own
  Ctrl+K and Ctrl+G), and each conversation in Slack's sidebar under
  Channels (`#name`) or Direct Messages, which goes there through the
  sidebar's own link (no reload). Only `jump`, `search` and `open http…`
  pass the host. 3 ms from asking to answer.
* **Agents and Passwords**: as in any of Olo's Chrome's windows (the
  extension's trees, Passwords and passkeys).

* **Status**: Slack's follows Olo's. The host follows the screen's lock
  (app_server's power events), Do Not Disturb (Notifications' state
  file, by the kernel's events; until when, from its settings, if it
  was turned on for a while) and calls (an app recording from a
  microphone, `olo_system::audio::watch_listening`: not Olo's own, not
  a monitor; for 20 s before it's a call, a voice clip isn't, and over
  after 10 s with none), and tells the extension `{slack: {status:
  {away, dnd, dnd_until, call}}}` when it changes (`status_of`); the
  background keeps the latest for a page that comes later. The page
  (slack-page.js) calls Slack's web API as its own client does, with
  each signed-in workspace's token from `localConfig_v2` and the page's
  sign-in: `users.setPresence` away while locked and automatic after,
  `dnd.setSnooze` for Do Not Disturb's time (a day at a time when it
  has no end) and `dnd.endSnooze`, and "In a meeting"
  (`users.profile.set`, :spiral_calendar_pad:) during a call, only when
  the person has no status of their own, cleared after only if it is
  still ours. What Olo set is kept in the page's `oloSlackStatus`, so
  only that is undone, after a reload too. Checked: `status_of`'s test
  (a lock, Do Not Disturb, a voice clip that isn't a call, a pause
  that doesn't end one), and the page's calls against a fake Slack in
  node (`node tools/dev/slack-status-check.mjs
  apps/chrome/extension/slack-page.js`: each case's calls, a status of
  the person's own left, ours changed by them kept), and
  `watch_listening` against a private PipeWire (a virtual microphone
  recorded counts; a speaker's monitor recorded doesn't); not yet against
  Slack itself, which would change the person's status where others
  see it.

Idle, nothing runs (the host 0% of a core, 0 wakeups a second, its
watchers asleep on their sources; the page watches only its
`<title>`). Test with a stand-in page served from
127.0.0.1 (`OLO_SLACK_URL=http://127.0.0.1:PORT/client`: a sidebar of
`/client/T/C` links, buttons that make a `Notification`, change the
title and call `setAppBadge`, a `target=_blank` link, a picture), a copy
of Chrome in a fixture `XDG_DATA_HOME`, `CHROME_CONFIG_HOME` and
`XDG_CACHE_HOME` in `/var/tmp`, in a tsession with its own bus and
`olo-notifications --bus` (its app id is the stand-in's there: check
`olo.slack` against Slack's real address, which shows its sign-in).

## Log Viewer (apps/logs)

`olo-logs` shows every Olo program's log in one place: what they
write to stderr goes to the user's journal (through the session's
journal stream, or, for a program started by another with its stderr
on /dev/null, its own: "Crash reports"), and Log Viewer reads that
journal natively. `olo-logs --query TEXT` opens it on that search (and
a running one takes it: Olo Doctor's "Viewer crashed" does). Kit apps
log `warn,usvg=error,olo=info` unless `RUST_LOG` says otherwise
(`olo_ui::run`; tsession starts clients with `RUST_LOG=warn`): Olo's
own crates keep their info for what is rare and worth knowing after
the fact (a reconnect, an upgrade, why a close was asked about: the
Terminal's `asking before closing` names the program and where the
close came from, app_server's `asked to close a window` and `close box
clicked` the window and who), dependencies only their warnings.

Logs an app writes beside the journal are read too: File ▸ Agent Screen
Logs lists the Agent Screens' (`$XDG_RUNTIME_DIR/olo-screens`: a
screen's own `NAME.log` and each program run there, `NAME-PROGRAM.log`,
the latest written first), File ▸ Open Log File… (⌘O) any other, and
`olo-logs FILE` (or Tracker's Open With) opens one; File ▸ Journal goes
back. The window then shows that file, titled by it, with the same
search, filters and live tail (`reader::run_file`: read whole, then
followed as it grows, from the start again if it is cut short, as a
screen started again under its name does). Its lines are parsed as the
journal's (`model::file_line`: tracing's time and level, Chromium's
prefix, GLib's words, a panic as an error, indented lines going on with
the one before); those without a time of their own take the file's.
Changing what is read restarts the reader with a new generation
(`Logs::start`): the old one stops, what it already sent is dropped, and
the list's kept cells (keyed by line) are keyed by the generation too.
`olo-logs --print --file LOG` prints one for agents.

- journal.rs: systemd's journal file format (JOURNAL_FILE_FORMAT.md): the
  header, entries through the chain of entry arrays, fields in data
  objects (zstd, xz or lz4; the regular and the compact layout). Data
  objects are cached by offset (most are shared between entries). A
  file still being written is read on from where it was left, and
  across journald rotating it (the open file is finished, then the new
  one opened). `user_files(uid)` finds `user-UID.journal` and its
  archives in /var/log/journal (or /run/log/journal), oldest first.
- reader.rs: on a worker thread, every file in batches, then the active
  one read on as it changes (`olo_ui::watch`: journald truncates the
  file to its own length after each write so that readers hear of it;
  every 30 s anyway, every ½ s without inotify); `Update::Lines` carries
  new lines and more text for lines sent before.
- model.rs: an entry as a line: the program (`_EXE`'s name, else
  `_COMM` made whole from a longer name seen, else the identifier;
  Chrome's lines, which Google's launcher passes through `cat`, as
  chrome), the level and text without terminal colours and without
  `tracing`'s time and level (or Chromium's `[pid:tid:time:LEVEL:file]`),
  GLib's `-WARNING **` and panics recognised, and Olo's own stderr
  lines Info unless they say otherwise (the session's stream files all
  of stderr as warnings). Continuation lines (a pretty debug print, a
  backtrace) join the line before from the same process. `Query`, made
  from the search's text (`Query::from_text`, the kit's language,
  `olo_ui::search`): `since:` (10m, 2h, 1d, today, boot, all; since
  startup if not said), `level:` (the least: debug, info, warning, error;
  info if not said), `program:` (several, any of them, matched by their
  start while one is typed; `all` for every program; none: Olo's),
  `-program:` (left out), `pid:`, and words, phrases and `-words` in the
  message or the program.
- highlight.rs: syntax highlighting as spans: the target `tracing` puts
  first, `key=value` keys, strings, numbers, paths and URLs, type names,
  trouble words; the list and the detail pane colour them from the
  palette.
- main.rs, on the GPUI kit (olo-ui): the kit's `SearchField` and a `FilterBar` (Program, with
  each program's lines in the query's time and Olo's first; Level; Time)
  above a `ColumnListView`, in one row when the window has room for the
  field, else the menus under it (their text over the list's first
  column; the Message column as wide as the list allows, beside its
  scroll bar); what the words find is marked in the messages
  (`search::runs`: the syntax colours, the marks behind; `Cell::text_runs`).
  The selected line in full (highlighted, selectable, a `TextView`)
  below; Live keeps the list at its end as lines arrive while it is
  there (`ListModel::at_end`); a new query is gone through 2 ms at a
  time between frames (a task yielding to them), and a sliding time
  ("Last 10 Minutes") is chosen again every five seconds only while
  it's asked for. Loading 160,000 lines its worst frame is 3 ms
  (egui's 34), a live line one frame of under 1 ms to 3 (egui drew
  three a line), and it holds 17 MB less. Show in Context
  (⌘K, or a line's context menu) shows every line around one
  (`program:all level:debug since:all`), and Back returns to the search
  before. Show Only (a line's menu) sets `program:` to its program. The
  search's filters are kept from one run to the next, its words aren't
  (`search::filters_only`); settings from before (a menu each) become
  the text once.

For agents, the same query from the command line, reading the journal
directly: `olo-logs --print [--query TEXT] [--since
10m|2h|1d|today|boot|all] [--level debug|info|warning|error] [--program
NAME | --all-programs] [--find TEXT] [--json] [--follow]`, `--query` as
typed in the window's search and the others its filters (JSON lines:
time in UTC, program, pid, level, message). Tests: `cargo test -p olo-logs` (synthetic journal files
in both layouts); `-- --include-ignored` also reads this machine's
journal and highlights every line in it.

## Deskbar (apps/deskbar)

The Olo menu, the tray and the running applications, in the top-right
corner of every display (a layer surface each, namespace `deskbar`, no
keyboard: its menus take the keys as they open), on the GPUI kit. One
model (`Deskbar`: app_server's windows, workspaces and displays as
events say, the tray's status, other programs' tray items and launcher
badges) that every display's bar (`BarView`, which keeps only what the
pointer is doing over it) draws: a bar draws again when the model
changes, the pointer moves onto something else in it (only that bar),
a dot shakes or drains, or the clock's minute (or second) comes; idle,
nothing runs. Each bar is painted in one `canvas` (the egui drawing,
moved over: the tile and wordmark, the tray's well, headings, rows,
the lit workspace), with transparent hit areas over it for the pointer:
a left click as a click, the right and middle buttons as they're
pressed (menus open on the press, as BeOS's), hovering that lights and
a tip in a popup of its own (`olo_ui::tip`: the bar is narrower than
what its icons say). A tray icon opens its menu on either button (Ben,
2026-11-06), each menu ending with the app that tells more (Network's,
Displays', Power's, Sound's, Notifications' and Security's preferences,
Activity Monitor, Screenshot); one with no menu (Olo Doctor, the agent
screens, an upgrade) does what its click does on either; another
program's item shows its menu on either button, with "Open NAME" (its
activation) last, or is activated if it has none. The volume's wheel
still sets the volume; Mute is in its menu. The tray's glyphs (status.rs) are exact-coverage
paths and the GPU's discs, rings and arcs (`paint_sector`), straight
lines on device pixels. An application dragged out is a link to its
program (`olo_ui::dnd`, its icon and name under the pointer); files
dropped on one open with it, its row lit as they come over it. A
click on a running application's row with ⌘ held (Super on a Mac's
keys; Control too), or ⌘ held as an application is chosen in the Olo
menu (`run:` and `desktop:` items), starts it again as a process of its
own beside the one running (`launch_again`,
`DesktopEntry::launch_new_instance`): Olo's apps are started with
`OLO_NEW_INSTANCE=1` (`olo_xdg::NEW_INSTANCE`), for which the kit's
`instance::forward` hands nothing over (and clears it; `spawn_detached`
never passes it on), so the copy opens its own window and leaves the
launch socket to the first; another program does as it does (Chrome
opens a window in the one it has). Deskbar never has the keyboard, so
Wayland doesn't say what's held: it asks app_server at the click
(`Request::HeldModifiers`, the seat's keys as pressed). Tried headless:
⌘-clicking Terminal's row, and ⌘-choosing Terminal in Applications,
each started a second olo-terminal with its own window; plain, a
window in the one running. Bars
for displays that come are opened once the model's update is over (a
window draws as it opens, and its bar reads the model), before those
going are closed, so the app always has one. Approve Without Asking's
window (approve.rs) is the kit's alert frame with a `MenuField` and a
`ConversationField`. Release build, against the egui one: first frame
58–73 ms (egui 69–87), 57 MB resident (141), 30 pointer moves across it
32 frames, 0.8 ms each (egui drew 150). The tray's items are counted
before they're placed (one more than counted is drawn nowhere and
logged, never a crash: Olo Doctor's bag, uncounted, took Deskbar down
on its first start after an upgrade, 2026-11-04). The battery: a click
opens Preferences ▸ Power, a right-click the power mode (Performance,
Balanced, Power Saver, checked as the daemon says) and Power
Preferences…. Olo Doctor's bag shows while its last look found
something to fix (`olo_protocol::doctor::DoctorFound`, followed by
inotify); a click opens Olo Doctor. The speaker's menu has None first
among the outputs (below). The tray's battery and links
follow UPower's display device and NetworkManager's signals
(`olo_system::battery::watch`, `network::watch_links`, each on a thread
that sleeps until one comes; the links on a bus connection of their own
that the bus sends only device state changes, NetworkManager's own (the
default route moving), and for each link connected its properties, its
addresses' and its access point's: a scanning adapter's access points
come and go several times a second here, and aren't asked for), so idle
it wakes 0 times a second (it was 7–9, polling every 4 s). A signal
that says only the access point's strength or the adapter's rate (what
comes every few seconds) is taken from the signal's own values
(`network::quick`); anything else reads the links again (about two dozen
calls, 3.5 ms here) and asks for the signals of what's connected again
only if that changed. Measured beside the old Wi-Fi-only watch over
60 s, idle on Ben's machine (Ethernet and Wi-Fi both up): under one
wakeup a second, against about two.

### Live widgets (widgets.rs, Preferences ▸ Deskbar)

Ben, 2026-11-07: "add a preferences app for the deskbar and in it
support configuring/adding live performance widgets into it e.g. cpu,
gpu, disk, network monitors etc. -- it should all be very customizable
in terms of where they appear on the deskbar, how they look, how often
they update, exactly what to show and so on". As BeOS's Pulse and
ProcessController put their bars in its tray.

* **Settings**: `Settings::deskbar` (`olo_protocol::deskbar`), kept by
  app_server, so Deskbar follows a change at once (`SettingsChanged`)
  and agents read and set them (`GetSettings`/`SetSettings`; a widget
  with only `id`, `measure` and `place` takes the rest as it comes).
  Each `Widget`: an `id`, what it measures (`Measure`: CPU, all or each
  core; memory, and swap; a GPU by PCI address; a disk's reads and
  writes; a volume's fullness; an interface's down and up; a hwmon
  temperature, "chip/label"; battery power), where (`Place`: an icon's
  place in the tray, a row under the tray, a row at the foot under the
  applications), its look (`Look`: graph, bars, meter, number), its
  row's size, its name and value shown, how often it's read
  (`every_ms`, 250 ms to a minute), how far back its graph looks
  (`span_s`) and its colour (a `Hue`, or the accent).
* **Readings**: `olo_system::meters` (Preferences' list of what there
  is too, `sources`: cores, GPUs, disks, volumes, interfaces, sensors,
  a battery): `/proc/stat`, `meminfo`, `diskstats`, `net/dev`, statvfs,
  hwmon, the battery's `power_now`, and the GPUs through
  `olo_system::gpu_load` (Activity Monitor's, moved there for both: a
  GPU asleep is never asked, an idle NVIDIA one only every 20 s, each
  question being use that keeps it awake; xe's busy is its render GT's
  idle residency). A worker (`deskbar-widgets`) reads each widget when
  it's due and sends them together (`Msg::Meters`); with no widgets
  there is no thread, and with the displays asleep or the screen locked
  nothing is read (`Live::pause`). What a widget draws is made once a
  reading (`Shown`: each series' levels over its history, rates against
  the highest in it, its value in words), not each frame.
* **Drawing**: tray widgets take the tray's first slots (a graph, bars
  or meter in a sunken well, or the value in small figures); rows have
  the name at the left and the value at the right (shortened, "↓27K
  ↑102K", or without the name, when both don't fit), the drawing
  between them in a small row and under them in a medium or large one.
  A tip says what it measures in full and each series' value; a click
  opens Activity Monitor; the other button, Activity Monitor and
  Deskbar Preferences…. Agents read each as a `Meter` with its value.
* **Cost**, over 20 s on release builds (`tools/dev/cost`): no widgets,
  0.1% of a core, 2 wakeups a second, 0.2 frames a second; eight (most
  read every second), 0.2%, 5 wakeups (2 the worker's), 1.1 frames a
  second at 613 µs (430 without).

Preferences ▸ Deskbar (`deskbar.rs`) lists the widgets in order (Add
one of a kind after the chosen one, Remove, Move Up, Move Down) beside
the chosen one's settings (what it shows and which of it, where, look,
size, name and value, how often, how far back, colour), each changing
the Deskbar at once; Defaults has none.

## The lock screen (crates/app_server/src/lock.rs, apps/lock)

`lock_screen()` (Ctrl+⌘Q, Deskbar ▸ Lock Screen, `Request::Lock`, the
display turning off for idleness when `power.lock_on_display_off`, logind
`PrepareForSleep` and `Lock`) covers every output in black at once, takes
keyboard focus away and starts `olo-lock` on a socket pair of its own
(WAYLAND_SOCKET): ext-session-lock is offered to that one client only. The
locker is restarted if it dies (at most every 2 s); the screen stays
black meanwhile, so the lock never fails open. While locked, input reaches
lock surfaces only, keybindings are off, and the displays sleep after a
minute. A marker, `$XDG_RUNTIME_DIR/olo-locked-$WAYLAND_DISPLAY`
holding olo-session's pid, makes an app_server that reloaded or crashed
come back locked (only in the same session: display names get reused). Before sleep,
app_server holds a logind delay inhibitor until every output shows the lock
surface (or 3 s).

`olo-lock` (on olo-ui) uses `olo_ui::lock_session(cx, build, on_event)`
(a window from `build` on every output, and on any added while locked;
`Locked` once, `Finished` if refused; the first output's window has the
panel) and `olo_ui::unlock_session()` (sent at once, then quit). GPUI
takes app_server's `WAYLAND_SOCKET` as a Wayland connection, and a lock
surface makes no commit before its first configure and checks the password with `olo_system::auth` (PAM via
libpam, on a thread) while listening for Touch ID with
`olo_system::fingerprint`. Debug builds take `OLO_PAM_CONFDIR` +
`OLO_PAM_SERVICE` (passed through by tsession), so tests use a private
pam_exec service and never a real password; then (or with
`OLO_NO_FINGERPRINT=1`) it leaves the fingerprint reader alone, which is
the person's own:

```sh
printf 'auth required pam_exec.so expose_authtok quiet /path/check.sh\naccount required pam_permit.so\n' > $dir/exec
OLO_PAM_CONFDIR=$dir OLO_PAM_SERVICE=exec tools/dev/tsession start lk --light
tools/dev/tsession ipc lk '{"type":"lock"}'; tools/dev/tsession type lk secret; tools/dev/tsession key lk 28
```

## Olo Rescue (app_server rescue.rs, apps/rescue, `olo_protocol::rescue`)

Holding Ctrl+Shift+Super and pressing Delete (or Backspace, a Mac's
delete; either side's modifiers), from anywhere, whatever has the
keyboard, brings Olo Rescue: for when something has gone wrong and the
person can't see or can't drive the desktop. app_server reads the chord
on its input thread from the keys' physical codes, first of all
(`physical_key`: before the keymap and its profile, shortcuts other
programs took, keyboard hints, the overview, a layer surface that takes
every key), so no setting, remapping or program can hide it; the
Delete is seen by nobody, and Super let go after it is no tap (no
palette). It costs a key 1.4 ns, and checking whether Rescue is on (each
layer surface, each frame and pointer move) 0.32 ns
(`rescue::tests::rescue_cost`).

In Rescue:

* **Everything is put away.** Each display shows a workspace of its own,
  one past the person's (the count is one more meanwhile); only
  Rescue's own layer surfaces (namespace `olo-rescue`) are drawn,
  reached or given the keyboard (no Deskbar, desktop, banners or a
  layer that took every key), nor a full-screen window, nor the
  wallpaper. Shortcuts that would leave the workspace or open what it
  doesn't show are off (`rescue::in_rescue`: other workspaces, the
  overview, Show Desktop, hiding, the palette, hints, Find, a
  screenshot's selection); ⌘Tab switches among what's open in Rescue.
  Windows opened in Rescue (Preferences, say) open on it, over Rescue's
  surface (it's on the Bottom layer).
* **app_server runs on safe settings** (`rescue::safe`): each display on
  its own at its preferred mode and the largest of 2×, 1.5× and 1× that
  leaves it 1280×800 points (`safe_scale`: Olo's default 2× is too big
  for a 1080p monitor), unrotated, unmirrored, no VRR, HDR or Vivid;
  the built-in display the main one; night light off, no wallpaper, no
  screen saver; a screen at a quarter of its brightness or less
  brightened (and kept so); the keyboard's layout, variant, profile and
  terminal and browser apps kept (it types, and shortcuts are where the
  hands know them), its repeat, xkb options and hints, and the
  pointer's settings, as Olo comes.
* **The person's own settings are what clients see and change**:
  `GetSettings`, `SettingsChanged` and `Outputs` tell them (Outputs:
  each display's saved scale, rotation, mode, place, mirroring and
  colours, so Preferences ▸ Display shows what broke), and
  `SetSettings` and `ConfigureOutput` change them, saved at once, applied
  as Rescue ends (`AnvilState::told_settings`, `settings_changed`). So
  agents and tools reading `Outputs` in Rescue get the saved scale, not
  the safe one on screen (tsession's crops are then off).
* **olo-rescue** (apps/rescue) covers each display with a layer surface:
  on the main one, Rescue itself, on the others "Rescue is on …" and
  Show Rescue Here (only Rescue's takes the keyboard, and app_server
  gives it it as it shows). It asks rtkit for `RESCUE_NICE` (−15, over
  the compositor even) before it starts a thread, and depends on nothing
  of the session but app_server: a terminal of its own (term.rs:
  alacritty_terminal's emulator and PTY in process, `bash --noprofile
  --norc` so a broken profile or login shell can't take it; ⌃⇧C and
  ⌃⇧V, which the keymap makes ⌘C and ⌘V, as Rescue is a terminal to it;
  the wheel and ⇧Page Up/Down for the scrollback; selection by the
  pointer; deliberately apart from Terminal, no olo-termd or saved
  windows), the person's processes (busiest first, read on a thread
  every two seconds while it's up; End and Force End), Preferences'
  panels (`olo_xdg::apps::PREFERENCE_PANELS`; what's changed applies as
  Rescue ends), Reset Display Settings… (every display Olo's own again,
  brightness kept), Start Over (`olo-session reload`: Rescue stays on),
  Log Out… and Restart…, and Leave Rescue.

Pressed in Rescue, the keys bring olo-rescue back if it had gone. While
locked they change the displays (a lock screen must be seen to be
unlocked) and olo-rescue comes once the screen is unlocked: Rescue is
no way past the lock. Leaving (`Request::Rescue { on: false }`, Leave
Rescue) brings the person's settings back, each display's mode,
colours and VRR, and what was opened in Rescue onto the workspace its
display showed before. An app_server restarted in place in Rescue stays
in it, each display's workspace from before kept (handover.rs), and the
session starts app_server in Rescue (`OLO_RESCUE`) once it has crashed
twice in a minute, as a setting may be what it can't start with.

Agents: `{"type":"rescue","on":true}` and `false`, and `rescue` in
`PowerStatus` (and `PowerChanged`). `tools/dev/rescue-check` checks it
all in headless sessions: the keys (both sides, Delete and Backspace),
its shell taking the keyboard at once, each display's scale (2×, 1×,
1.5×) and the layout review on each, Leave Rescue clicked (the window
and the saved scale back), the keys while locked, and a restart in
place.

## Passwords (apps/passwords, crates/onepassword)

**Searching**: the toolbar's `SearchField` and a `FilterBar` over the
list (Tag, Has) share one query (`olo_ui::search`). Words find an item
by its title, username, websites, tags, kind, its sections' titles, its
fields' labels (a secret's too), the values of those that aren't secret,
and its notes; with words, the best come first (title 4, username and
site 3, tags 2, the rest 1), else by title; what was found is marked in
each row, whose second line is the username (else the site), or, when
the search found it inside the item, where: "Admin Console › region:
eu-west" (the field with most of the words; `found_in`, worked out with
the list, once per change). The item pane marks it too (section titles,
labels, and values that aren't secret). Filters: `type:` (login,
secure-note…), `vault:`, `tag:`, `site:`, `user:`, `has:` otp, password,
website, files, reused (its password is another item's too), weak (under
12 characters, or of one kind), and `section:` and `field:` (by title or
label, suggested with how many items have each), which narrow the item
pane as well to just those sections and fields (`-section:` and `-field:`
leave them out), saying so under its header. The vault worker sends a
`Searchable` for every item with the list (`store::index`, again after
each change): the username, each field as `FieldText` (its section's
title, its label, its value unless secret), that and the notes as text,
and flags; never a password, code, card number or key, and of a
password only whether it is weak or reused. So searching finds usernames
of items never opened.

**Editing**: the editor shows the fields as the item pane does: the
built-in ones and untitled sections' first, then each titled section
under its heading (one with no fields yet too); each group ends with its
own Add Field…, whose field goes at the end of that section (the first
group's in the untitled section custom fields go in, made when first
needed; `Editor::groups`, `add_field`) and is focused: its label if it
has none yet, else its value. A passkey's section has none.

`crates/onepassword` (package `olo-onepassword`) is a Rust 1Password SDK
modelled on the Go one: the same `Client` (`Secrets`, `Items`, `Vaults`,
`Groups`, `ItemShares`, `ItemFiles`, `Environments`), the same types and
JSON shapes (camelCase, tagged enums), `op://` references, the password
generator (random, memorable from the EFF word list, PIN) and TOTP. It
talks to the 1Password desktop app's SDK integration, as the official SDKs
do, over the abstract unix socket `@1PASSWORD_SDK_INTERGATIONS` (sic): one
connection per call; frames are a u32 LE length (counting the flag byte),
a flag byte (2 = more, 1 = last) and up to 499,999 bytes of a JSON request
`{kind: init_client|invoke|release_client, account_name, payload: base64}`;
the reply is `{success, payload: base64}` whose payload is the result's raw
JSON (or `{name, message}` on failure). 1Password asks the user to allow
each app and unlocks itself; users turn on Settings ▸ Developer ▸ Integrate
with other apps. Service-account (token) access is not implemented: that
needs 1Password's closed core.

`onepassword::accounts::local()` lists the accounts signed in to the
app (name, email, sign-in address, UUID) from the summaries in its
`1password.sqlite` (`~/.config/1Password`, or the Flatpak's;
`OLO_1PASSWORD_DATA` overrides), read-only and nothing else from it.
Passwords' setup offers them as a choice and keeps the UUID, which the
SDK integration takes as the account's name.

Tests never touch the real app. `onepassword-fake [--account NAME]
[--socket NAME] [--data FILE] [--accounts DIR]` (DIR gets a
`1password.sqlite` listing it) serves an in-memory account (two vaults of
sample items, item versions, OTP codes) over the same protocol;
`tests/desktop.rs` drives it. Apps take `OLO_1PASSWORD_SOCKET` to use it.

Passwords is the app: sources (vaults, categories, Archive), a two-line
item list and the selected item's fields (hover for Copy/Show; one-time
codes count down), an editor with templates per category, a Password
Generator panel, and File/Edit/Item/View menus (⌘C username, ⇧⌘C password,
⇧⌘O one-time code, ⌘0 All Items, ⌘1–⌘9 the vaults by place, as
`app_keys` has them). Secrets go to the clipboard with
`olo_ui::copy::secret` and leave after `clear_seconds` (quit before,
Passwords runs on without its windows until they do). A field (or website) drags out as text onto
another app's field (`drag_field`: a one-time password's current code;
the picture shows the field's name, never the value). It opens locked and locks after
`lock_minutes` idle, and when the screen locks if `lock_with_screen`
(`lock.rs`): Touch ID through fprintd, claimed only
while the locked window has the keyboard, or the account password through
PAM. Those, and `auto_sync`, are Preferences ▸ Security's
(`Settings::security.passwords`, followed live through
`olo_ui::settings::watch`; the screen locking comes from
`settings::watch_power`, `PowerStatus::locked`; View ▸ Lock After writes
the same setting with `settings::update`). Its own settings (accounts,
the generator, the window size) live in `passwords.json`.

It is on olo-ui: the app's state is a `Passwords` entity, which is the
main window's view; the menu bar and the three panes are cached parts
of it (`olo_ui::part`), so a one-time code counting down (a timer on
the second, only while one shows) draws the item pane alone. Nothing
ticks: one timer waits for the next thing due (a sync, the auto-lock,
the status line clearing), set again whenever the app changes
(`observe_self`). The list is worked out once for each change to the
items, the source or the search (`visible`). Review Sync, Sync
Problems, the generator (a panel: it floats, never a tab) and the
alerts are views of their own that read it; a passkey question closes
itself when the page gives up. Debug builds only take the PAM test
service, so a release build can't be unlocked in a test session: to
measure one, build it with `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true`
in a scratch `CARGO_TARGET_DIR`. Tests give it a keychain socket of its
own (`OLO_KEYCHAIN_SOCKET`; the default is the person's Passwords') and
a fake 1Password on a socket of its own (`onepassword-fake --socket
NAME --accounts DIR`, `OLO_1PASSWORD_SOCKET=NAME`,
`OLO_1PASSWORD_DATA=DIR`).

**Its own store** (`vault.rs`). Every vault and item lives on this
computer, so the app, Chrome's extension and the palette never wait on
1Password: `passwords.vault` in the config directory, "TABV", a format
byte, a 24-byte nonce and the JSON sealed with XChaCha20-Poly1305 (header
as associated data), written whole and atomically (0600) after each
change. The key is 32 random bytes in the login keyring
(`olo_system::secrets`, a Secret Service client that first checks a
keyring owns, or can start, `org.freedesktop.secrets`, so with none it
answers "nothing" at once; the session bus's calls time out after 10 s;
PAM unlocks the
keyring at login, so Touch ID still opens Passwords). A store whose key is
missing is never overwritten with a new one. `OLO_PASSWORDS_KEYFILE`
keeps the key in a file instead; tsession sets it for every session, so
test apps never touch the real keyring. The worker (`store.rs`) owns the
store: it opens it at the unlock, lets it go at the lock (after a running
sync), and answers the UI, Chrome's extension and op from it. Items keep 1Password's
types; each entry also records what 1Password last had (version, update
time, archive state), whether it changed here, and a revision; deletions
of synced items leave tombstones. Without an account (Keep Only on This
Computer) items live in a local "Personal" vault.

**Syncing with 1Password** (`sync.rs`, optional: the setup panel or File ▸
1Password Accounts…) never changes anything, on either side, without the
person's approval. It runs in two steps, each on its own thread:

* `sync::plan`, from a snapshot (`Vault::snapshot` changes nothing, even
  for an account switch), only reads. Down: vaults whose
  `content_version` moved are listed (both states); new or changed items
  (update time or state) are fetched fifty at a time with `get_all`;
  items gone there are noted. 1Password refuses to give apps archived
  items ("item is not in an active state"), and one archived id fails a
  whole `get_all`, so only active items are fetched: an item archived
  there since is archived here from its listing (`Change::Archived`),
  archived items never synced are left out with a note, and whatever a
  batch didn't bring is fetched one at a time, so a bad item costs only
  itself. A field 1Password's SDK names but won't give the value of (its
  `Unsupported`, with no value and no section: an older card's expiry,
  three of Ben's cards) is asked for by its secret reference
  (`op://vault/item/field`, `resolve_unread`, all of a batch at once,
  else one by one), which 1Password answers: a card's expiry or
  valid-from that reads as a month and year becomes a `MonthYear`
  ("MM/YYYY"); any other keeps its value. An item synced before that
  with such a field empty is fetched again, once, its vault changed or
  not (`has_unread`). `cargo run -p olo-onepassword --example
  card_shapes [VAULT ITEM]` shows what the 1Password app gives for each
  card (or one item): every field's id, label, type and section, and its
  value's shape only ("NN/NNNN"), never a value or a title, and how its
  unsupported fields resolve. Up: 1Password's copies of items changed here
  are fetched to compare (their unread fields asked for the same way). The plan is the store's `Change`s, what to send,
  and a `Row` for every change either side would see (add, change with the
  parts that differ by name, never values, archive, delete, keep both).
* The store keeps the plan and the UI shows its rows in Review Sync. Only
  `Job::Approve` with that plan's id runs `sync::apply`, which makes
  exactly those changes. A changed item is put against the version that
  was reviewed, and a deletion checks it first, so anything changed in
  1Password meanwhile is refused (reported) rather than overwritten, and
  comes up in the next review. A plan nobody would notice (nothing but
  what was looked at) applies without asking.

With `auto_sync` (the default), plans are made at the unlock, every 5 minutes, 3 s after a change here
(the status line then says "3 sync changes to review"; a click opens
them), and on File ▸ Sync Now or the setup panel's Sync (the review opens
at once). Automatic plans wait while Review Sync is open. Cancel leaves
"Not synced: changes not applied". Changed on both sides: 1Password's copy
stays as the item and ours is kept beside it as "… (conflict)", to be
reviewed next time. A push that lands after another edit here keeps the
edit pending (revisions). The first sync with an account brings in
everything; local-only items move to its Personal (or first) vault.

**Several accounts.** The setup ticks any of the 1Password app's accounts
(`Settings::accounts`, `Linked { id, label }`; the first takes items made
here; an id is the UUID, or a name typed by hand, which the app also
answers to and which is kept). The store keeps which account each vault is
in (`Data::vault_account`); `Vault::snapshot(account, home)` holds only that
account's vaults (and, for the first, the local vault), so one account
never sees another's vaults as gone. `Job::Sync { accounts }` plans each
in turn into one `Plan` of per-account parts, one review: with more than
one account a vault is named "Personal (Work)" there, in problems, the
editor and the item's header, and the sidebar lists each account's vaults
under its name (as Tracker lists volumes). An account that can't be
reached is a problem, not a failure, unless none can be. Unticking one
asks first, then `Job::RemoveAccount` drops its vaults and items here
(never-sent items stay, in the local vault). Stores and settings from the
one-account days carry over (`Data::account`, `Settings::account`). The
fake serves more signed-in accounts on one socket (`FakeAccount::others`). Files attached in 1Password aren't copied (their metadata
is), so they aren't shown.

**Problems** are `sync::Problem`s (vault, item or none, a reason in plain
words from `sync::reason`, and whether it's only a note), never joined
text. The status line counts them ("Synced just now · 2 problems", red
only for failures); the review shows one line ("⚠ 2 problems reading
1Password…"). Either opens Sync Problems: grouped by vault
(`sync::grouped`, repeats dropped), a warning or info icon, the item and
its reason under it, with Copy (`sync::as_text`) and Sync Again. The fake
behaves as 1Password does: archived items can't be read, and a vault with
`"no_access": true` can't be listed.

**Reset** (File ▸ Reset Passwords…, after an alert) erases the store here
(`Vault::reset`, or the file if it can't be opened) and the account
choice, back to the setup panel. It never touches 1Password: pending
deletions go with the rest.

`cargo test -p olo-passwords` covers the store (sealing, wrong keys,
conflicts, races, accounts, reset) and syncing against the in-process fake
(both ways, conflicts, nothing to do, 1Password down, that a plan
changes nothing and names every change, archived items and an
unlistable vault, and several accounts in one review). The Secret Service
client has an ignored test for a private keyring:
`tools/dev/private-keyring start` (busd if installed with `cargo install
busd --root /tmp/olo-dev/busd`, else dbus-daemon, plus its own
gnome-keyring), then
`DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/olo-dev/keyring/bus cargo test
-p olo-system secrets -- --ignored`; it refuses any other bus.

```sh
target/debug/onepassword-fake --socket olo-fake &
tools/dev/tsession start pw 2400x1600 --light
tools/dev/tsession run pw OLO_1PASSWORD_SOCKET=olo-fake OLO_NO_FINGERPRINT=1 \
    OLO_PAM_CONFDIR=$dir OLO_PAM_SERVICE=exec target/debug/olo-passwords
```
(`run` doesn't pass `OLO_PAM_*` on by itself; give them as above, with
`$dir` set up as for the lock screen.) Account: `Olo Test` (choose
Sync in the setup); password: whatever `check.sh` accepts. Pass
`--data FILE` to the fake to see what reached "1Password".

### op: 1Password's command line (apps/op)

`olo-op` stands in for 1Password's `op` so scripts that use it (`op
inject`, `op read`, `op run`, `op item get`, `op whoami`, `op account
list`, `op signin`) work from Passwords. It is installed with an `op`
link to it beside the other binaries (`~/.local/bin`, ahead of
`/usr/bin` on the PATH); `olo_xdg::link_op` makes the link only where
there is no `op` already and removes only its own. app_server keeps the
link in step with Preferences ▸ Security ▸ "Answer 1Password's op
command" (`PasswordsSecurity::op_cli`, on by default) at startup and on
each change, skipping `target/` builds; Passwords refuses `op` while it
is off.

Passwords is the session's SSH agent (`agent.rs`; Preferences ▸ Security
▸ "Be the SSH agent", on by default): the agent protocol
(draft-miller-ssh-agent: identities and signatures; adding, removing
and locking refused, Passwords keeps the keys) on
`olo_xdg::ssh_agent_socket()` (one a display), which olo-session puts in
`SSH_AUTH_SOCK` as a login starts (unless the login gave one) and
exports to systemd and D-Bus activation. olo-session starts `olo-passwords
--agent` at login (`REAL_SHELL`): Passwords without a window, staying
when its window closes (`stay_resident`), opened by a launch as before;
off, it exits `SERVICE_NOT_WANTED` and isn't started again. Each request
of a program of the person's own (`SO_PEERCRED`) is a keychain request
(`Request::SshKeys`, `SshSign`, made in process with
`Incoming::local` and the program's pid), so it's answered as `op`'s:
once unlocked (an agent-only Passwords opens its window to be unlocked),
a signature once that terminal session is allowed ("Allow “ssh …” to
sign in with your SSH keys?", one grant with `op`'s), the keys read and
used only on the store's thread (`agent::keys`, `agent::sign`): SSH Key
fields of active items, OpenSSH text or PKCS#8 (Ed25519, RSA with
SHA-256/512 as asked, ECDSA P-256), through RustCrypto's `ssh-key`.
`~/.ssh/config` naming another agent (1Password's app writes
`IdentityAgent ~/.1password/agent.sock`) is said in Security, with "Use
Passwords for ssh" once this session's `SSH_AUTH_SOCK` is Passwords'
(`olo_keychain::ssh_config`: `IdentityAgent SSH_AUTH_SOCK`, the old line
kept as a comment). Tried headless with a store shared to a screen
(`ssh_fixture`): `ssh-add -L` lists its key, `ssh-keygen -Y sign` through
the agent makes a signature `ssh-keygen -Y verify` accepts. Idle, agent
only: 59 MB, no CPU.

`op` asks over the keychain socket (`Request::Cli { query: CliQuery }` →
`Response::Cli { json }`, in 1Password's JSON shapes), starting Passwords
if it isn't running; the shapes, field types and `op://vault/item/[section/]field[?attribute=…]`
references are `apps/passwords/src/cli.rs` (`cli::reference`, one
function for `op item get`'s and for Passwords' Copy Reference: an
item's right-click menu and Edit menu copy its main field's,
`cli::main_field`, its password else its first secret; a field's own
right-click menu, that field's; tested by resolving each back,
`copy_reference_resolves_back`). Account questions (`op
account list`, `whoami`) are answered from the linked accounts without
unlocking. Anything else waits for an unlock, then for the person's say:
an alert names the command, the program that ran it and its folder, and
Allow covers that terminal session (the caller's session id and
terminal, from `/proc/PID/stat`) until Passwords locks or for 10 minutes
after its last use (12 hours at most); Deny answers "not allowed".
`op run` passes the values in the environment and masks them in the
command's output (`<concealed by 1Password>`, `text::Masker`) unless
`--no-masking`; `inject` writes files 0600. Commands that change items,
vaults or accounts say Olo's op doesn't do them, and errors look like
op's (`[ERROR] date time message`, exit 1).

`cargo test -p olo-op -p olo-passwords` covers templates, env
files, masking, arguments, references and item JSON. To try it, run
Passwords as above and `op` from `target/debug/olo-op` through an `op`
link in a scratch folder with the session's `WAYLAND_DISPLAY` and
`XDG_RUNTIME_DIR`.

### The keychain (crates/keychain)

While Passwords runs it serves other Olo programs (Chrome's extension
host, the palette, `op`) on
`$XDG_RUNTIME_DIR/olo-keychain-$WAYLAND_DISPLAY.sock`
(`OLO_KEYCHAIN_SOCKET` overrides): one JSON request, one JSON reply per
connection, and only for our own uid running an `olo-*` program
(SO_PEERCRED + `/proc/PID/exe`); what's shared with agent screens
(`Shares`, `SetShares`) never to a program on one ("Passwords for agent
screens"). `olo_keychain::call` blocks, so call
it from a thread. Requests: `Logins {url}` (title and username of each
login whose website fits the page, honouring 1Password's autofill
setting; `Locked` if locked), `Fill {url, vault, id}`, `Check {url,
username, password}` (`Known`, `New` or `Changed`), `Save {…, replace}`,
and `Unlock`. Fill, Save and Unlock wait while Passwords is locked: it
comes forward and answers after the unlock. `olo_keychain::matches`
is the site rule, as 1Password's: the same registrable domain (the `psl`
crate's public suffix list, so mail.google.com's login fills
accounts.google.com but alice.github.io's never fills bob.github.io), the
same host where there's none (localhost, addresses), and never an https
login into a plain-http page (localhost aside). Every login that fits is
listed, nearest first (`olo_keychain::closeness`: the page's own host,
then its parent domains from the nearest, then the site's other hosts),
then by title; an item that can't be read is logged, not hidden silently.

## Token Pool (apps/pool, crates/pool)

(Its windows are on the GPUI kit, olo-ui: `ui.rs` holds the app's state,
a `PoolApp` entity that lives while the gateway does, and its window;
`clients_window.rs` the Clients window. Both carry the app's menu bar.)

Several Claude and ChatGPT subscriptions shared by Claude Code and Codex
on this computer, the way Token Pooler's Local mode does it on macOS
(written fresh; no team or cloud gateway). The session's real shell
runs `olo-pool --service` (no window; launching `olo-pool` again
opens it), which listens on `127.0.0.1:17891` (`OLO_POOL_PORT`), and
on `127.0.0.1:17892` (`OLO_POOL_PROXY_PORT`) as the clients' proxy.

- **Proxy** (`intercept.rs`, `authority.rs`, `peer.rs`): Claude Code
  and Codex run as on their own subscriptions, signed in as they are
  and talking to their own hosts, with `HTTPS_PROXY` set to the pool (no
  API mode, no other endpoint). It takes CONNECTs only. A connection to
  `api.anthropic.com` or `chatgpt.com` is opened with a certificate for
  that host from the pool's own authority (`pool-ca.pem` and its key,
  0600, in the config directory; name-constrained to those two hosts and
  issuing nothing further, so even its key vouches for nothing else),
  and on it the model requests (Claude's `/v1/messages…`, Codex's
  `/backend-api/codex/…`: `intercept::pooled`) are served as the gateway
  serves them; everything else on those hosts goes through untouched
  (the client's own sign-in, every header and byte, its compression,
  WebSockets and other upgrades joined byte for byte after the 101), and
  every other host is a plain tunnel. Only the clients themselves are
  opened: the program on the other end is read from /proc (the socket's
  inode in `/proc/net/tcp`, the process holding it; `peer::program`),
  and one that isn't Claude Code or Codex (a script one of them runs,
  which inherits the setting but needn't trust the certificate) is
  tunnelled, as without the pool. A model request asking for a WebSocket
  (Codex's Responses) gets 426, Codex's sign to use HTTPS for the rest of
  its session at once (other answers make it retry five times first):
  the pool serves an answer at a time from the account best placed for
  it. A provider with no usable account is tunnelled, so its client runs
  on its own sign-in. A request body compressed (Codex's zstd on
  ChatGPT's backend) is sent on as it came and read decompressed for its
  model (`gateway::plain`). The client's own account's headers
  (`x-organization-uuid`, `chatgpt-account-id` and the like) are the
  pool account's, or none, on a pooled request.

- **Gateway** (`gateway.rs`, axum): `/claude/v1/*` goes to Claude's API,
  `/codex/v1/*` to Codex's Responses API, each with the chosen account's
  OAuth sign-in in place of the client's (its `authorization`,
  `x-api-key`, cookies and hop-by-hop headers are never passed on;
  Claude gets the `oauth-2025-04-20` beta added). Only loopback `Host`s
  and no `Origin` (no web page can use it). An account's sign-in is
  refreshed within a minute of expiring, one refresh at a time per
  account, the rotated refresh token kept; a 401 is refreshed and
  retried once, then the account is marked signed out and the next
  tried; a 429 that says a meter ran out (Claude's
  `anthropic-ratelimit-unified-status: rejected`; any 429 from Codex)
  marks the account (or, for a per-model meter, just that model family)
  out until its reset and the next is tried. Claude's other 429s (a
  burst limit, or a request it won't serve on that account) are the
  request's: passed back as they came, the account left in use. Both happen
  before anything is passed back; a network failure is a 502 and never
  replayed. With none left, a 429 shaped as the provider's with a
  `retry-after` of the soonest reset (the clients are set to retry ten
  times). `x-olo-pool-account` names who answered. `/pool/status`
  is the state without sign-ins. Its own `/claude/v1/*` and
  `/codex/v1/*` remain for programs pointed at it by address (Olo's AI
  features).
- **Order** (`schedule.rs`): earliest-expiry-first. The account whose
  usable quota resets soonest serves, since quota that resets unused is
  lost; ties go to the older account, then the id. The account serving
  keeps serving unless another resets more than five minutes sooner.
  Windows can be model-scoped (Weekly Opus): a Sonnet request isn't
  held up by Opus's meter.
- **Usage** is read from the answers as they stream through: only
  `usage` objects (SSE lines containing "usage", or a JSON body's), the
  content discarded as it passes. Quota comes from every answer's
  rate-limit headers and the providers' usage reports, read for each
  account every five minutes (`SYNC_EVERY`; reset credits every 30) and,
  when a report answers 429, at twice the last wait up to 30 minutes:
  reading them twice a minute per account was itself rate limited.
- **Store** (`store.rs`): `pool.vault` in the config directory, sealed
  as Passwords seals its own (XChaCha20-Poly1305, header as AAD); the key
  is 32 random bytes in the login keyring (`OLO_POOL_KEYFILE`: a file
  instead, for tests). It is the pool's own, rather than Passwords'
  1Password-backed store, because the gateway must work, and save the
  refresh tokens providers rotate, while Passwords is locked. The vault
  is opened on a worker thread, retried every 30 s while the keyring
  is shut, and nothing is saved until it has been read.
- **Adding accounts** (`import.rs`): *this computer's* sign-in is read
  from `~/.claude/.credentials.json` (`CLAUDE_CONFIG_DIR`) or
  `~/.codex/auth.json` (`CODEX_HOME`). The pool then refreshes it, so
  the client's own copy goes stale: point the client at the pool.
  *Separately* runs the official client's own sign-in in Terminal with
  its config in a private directory on the runtime tmpfs (`claude auth
  login`, `codex login` with file storage), takes the result and deletes
  the directory. The same email again updates that account's sign-in.
- **Clients** (`clients.rs`): "Claude Code uses the pool" sets
  `HTTPS_PROXY`, `NODE_EXTRA_CA_CERTS` (the authority's certificate,
  added to what it trusts) and `NO_PROXY` (loopback) in Claude Code's
  `settings.json` `env`; "Codex uses the pool" sets `HTTPS_PROXY`,
  `SSL_CERT_FILE` (`pool-ca-bundle.pem`: the system's roots and the
  authority's, since Codex's setting replaces the roots) and `NO_PROXY`
  in Codex's `~/.codex/.env` (`CODEX_HOME`), which Codex reads as it
  starts. What was there before is kept in `pool-clients.json`.
  Unchecked, a client is only paused (`pool-clients.json`'s `paused`,
  mirrored in `Shared::paused`): its settings stay pointed at the proxy,
  which decides for each request (`Shared::routes`: not paused, and an
  account to serve it) whether the pool serves it or passes it through
  on the client's own sign-in (`intercept::through`; a new connection
  for it is only tunnelled). So checking and unchecking takes effect
  from the client's next request, with nothing restarted (Codex's
  service neither) and nothing under way cut. `olo-pool --put-back
  claude|codex` (`clients::put_back`) takes a client off the proxy
  altogether, its settings exactly as before the pool (from its next
  start). A client set up the old way (an endpoint of the
  gateway's own: `ANTHROPIC_BASE_URL` and the rest, or an `olo_pool`
  model provider in `config.toml`) is moved over as the pool starts
  (`clients::migrate`).
- **Codex's daemon** (`codex_daemon.rs`): Codex's interactive sessions
  run their turns in a daemon of Codex's own (`codex app-server`, pid in
  `~/.codex/app-server-daemon/daemon.pid`), which lives for days and
  reads `.env` only as it starts, so a window opened after the change
  still talks through a daemon from before it. Every 10 s, and right
  after the checkbox changes, the pool looks for a daemon that started
  before `.env` last changed (its start in ticks from `/proc`, which the
  pid file records too, so a reused pid isn't taken for it) and
  restarts it at once, even in the middle of a turn (an agent's turn can
  go on for hours, all of it straight to OpenAI; waiting for one to end
  once meant never), with Codex's own `codex app-server daemon restart`
  (its start, pid file, locks and updater are Codex's), run from the
  daemon's directory with its environment (its sessions' commands get
  theirs from it) less the pool's settings, which the new one reads
  from `.env` as it is now. Codex's windows reconnect to the new daemon
  by themselves and carry on with the conversation; a daemon merely
  stopped leaves them "Reconnecting to server…", since nothing starts
  another. Its conversations, and whether each is in a turn, are asked
  over its control socket (the app-server's JSON-RPC over a WebSocket:
  `initialize`, `thread/loaded/list`, `thread/read`: `threads()`).
  Since unchecking only pauses, a restart is needed only when `.env`
  first points Codex at the proxy (or `--put-back` takes it off).
- **Upgrades of the pool itself, without a gap** (`ui::hand_over`,
  `drain.rs`; Ben, 2026-11-06: "it should serve new requests in the new
  process but allow old requests to finish in their existing process so
  that nothing errors"). olo-ui tells the app a new olo-pool is installed
  (`olo_ui::on_upgrade_due`). The app starts a copy of the new version
  beside itself with the listening sockets (copies kept at bind,
  `Drain::keep`; made inheritable for it alone, `Drain::for_a_copy`, and
  named in `OLO_POOL_LISTENERS`, `PORT=FD`), the windows' state (the
  kit's `restore::hand_over`; its own windows close and the copy opens
  them), its launches (`instance::close`, then the copy listens:
  `OLO_NEW_INSTANCE=1`) and its pid (`OLO_POOL_PREDECESSOR`). The kernel
  gives each new connection to whichever process accepts on the shared
  socket, and the old one stops (`Drain::retire`), so the copy answers
  every new request at once. The old one finishes what it has under way
  (`Underway`, held by an answer's body until it has gone: `Counted`;
  the proxy's requests and the gateway's own `/claude/v1`, `/codex/v1`
  alike), closes connections as they fall idle (a client's next request
  opens one to the copy), and saves nothing (no quota reads, no activity,
  no whole-file writes: the copy owns the file). A sign-in either renews
  meanwhile goes under the store's lock file (`Store::lock`), after
  reading the file again for one the other renewed (`newer`: another
  token lasting longer), so it's renewed once; the old writes only that
  sign-in in (`Store::put_credential`), and the copy's saves keep a newer
  one from the file (`adopt_newer`) while its predecessor lives
  (`Drain::overlapping`). Claude rotates refresh tokens: renewing twice
  would sign the account out. Once nothing is under way (20 minutes at
  most: past the provider's read timeout), the old one goes: the
  olo-pool olo-session started becomes the **keeper** in place (same pid:
  `OLO_POOL_KEEPER`, `main`'s `keeper`), a subreaper (`PR_SET_CHILD_
  SUBREAPER`) asleep in `sigwaitinfo`, about 8 MB, that passes SIGTERM,
  SIGINT and SIGHUP on to the copies that serve and ends as the last
  does, with how it ended (so a crash restarts the pool as before); an
  upgrade makes it the new version's keeper in place. A later copy
  (`OLO_POOL_KEPT`) just exits, and its successor goes to the keeper.
  While the old one finishes, the kit marks it waiting
  (`restore::mark_waiting`: `$XDG_RUNTIME_DIR/olo-upgrade-waits/PID`),
  and olo-session's stale check leaves it alone rather than stopping it
  90 s after the upgrade. If no copy can be started, the old way: no new
  connections (they wait in the kernel's queue), what's under way
  finishes, then the process becomes the new version with the sockets
  (`hand_over`, `drain::listen`); 10 minutes at most, and an upgrade that
  never comes ends the drain (`Drain::end`). `/pool/status` says which
  (`drain`: `draining`, `retired`). Tested against a fake provider
  (`tests/drain.rs`: a new copy answering at once while the old streams;
  two copies renewing a sign-in once against a provider that refuses a
  reused refresh token) and end to end, real processes in a test session
  (`examples/seed_vault.rs` makes a made-up account; `OLO_POOL_UPSTREAM`,
  `OLO_POOL_PORT`, `OLO_POOL_PROXY_PORT` keep it apart from the person's
  pool; the binary run from a scratch copy, replaced and sent SIGURG as
  olo-session would): a 20 s stream under way finished whole while new
  requests were answered in the fake's own second; the old pid became the
  keeper; a second upgrade went to a third copy under the keeper; the
  window came back once; SIGTERM to the keeper ended all; a copy killed
  ended the keeper with it.
- **The Clients window** (Pool ▸ Clients, ⌘L, or the Clients button;
  `apps/pool/src/clients_window.rs`): the pool's activity over time, a
  line each for Claude Code and Codex in requests a minute over the last
  hour, six hours or day (the kit's `Graph`, stepped a minute or a few
  at a time, hovering says each one's rate; totals, failures and tokens
  in the legend), from `activity.rs`: each minute's requests by client,
  counted as each request ends and kept for a day in a plain file of its
  own (`state_dir()/pool-activity.json`, only counts, so not in the
  sealed vault), written at most once a minute. Below it, Codex's
  service (`running::scan`): its version, since when, whether it is on
  the current settings, where its connections go, its conversations
  (title, folder, working or waiting for you), and **Restart**
  (`codex_daemon::restart_now`, on a worker thread); then each Claude
  Code and Codex running (from /proc: `peer::Program::is_client`), its
  folder, since when, and where its connections go: to the pool's
  proxy, straight out to port 443 elsewhere, or none open (a Claude
  Code going only straight out while it should use the pool shows red;
  one with an MCP server can have both). What's running is read every
  two seconds while the window is open, off the UI thread. For agents:
  `olo-pool --clients [--json]` (the same, without the window),
  `olo-pool --restart-codex-service`, and the activity in
  `--status --json` (`activity`: the day's minutes).
- **Reset credits** (some subscriptions get a few, to reset their quota
  early): read with each sync (Claude's `cedar_ember` grants in the
  usage report, `?cedar_ember=1`; ChatGPT's
  `/wham/rate-limit-reset-credits`) and shown on the card. One is spent
  automatically when no account can serve and an account's
  *shared* quota (not just a model's meter) is what stops it, at most
  once a minute per account; or by hand (Account ▸ Use a Reset Credit…,
  after a confirmation). The redemption's request id is saved before
  the provider is asked, so a retry after an unclear outcome can't spend
  two. Claude's needs the account's organization (`organizationUuid`,
  read at sign-in). A credit due to expire within 48 hours is also
  spent before it's lost, at its best moment
  (`Account::spend_before_expiry`, checked after each sync): a reset
  gives back what the shared quota has used, so it waits while a later
  moment (just before the quota resets by itself, or 30 minutes before
  the credit expires) would give back more, projecting use from each
  window's rate (`Window::rate`: percent used over time since it began;
  its length known from its name); never for under 5%.
- **Activity** (a heatmap, after Token Pooler's menu, drawn as Pulse's
  LEDs with the kit's `CellGrid`): the requests open now are lit cells
  under the header (`Pool::open`, blue, marked C or X for Claude or
  Codex), and each card has a strip of the account's latest 25
  (`Account::recent`, saved): green completed, yellow rate limited (the
  next account took it), red failed (refused, a server error, a broken
  answer), grey cancelled (the client went away before the end). Each
  *try* is one cell: a request that moved on is yellow on the first
  account and green on the next. A finished answer is told from a
  dropped one by a marker chained on the end of its stream (`respond`).
  Clicking a cell shows its account, model, client (from the user
  agent), time, duration, status and tokens on the line under the grid,
  which is always there, so nothing moves.
- **Agents**: `olo-pool --status [--json]`, or `GET /pool/status`
  (`open`, each account's `recent`, and `served`, each try with its
  `outcome`).

Tests: `cargo test -p olo-token-pool` runs the gateway and the proxy
against a fake provider (`Upstreams::fake`, standing in for every host;
`OLO_POOL_NO_TUNNEL` refuses tunnels, so nothing reaches the network).
`tests/clients.rs` runs the real Claude Code and Codex through the proxy,
signed in with fake subscriptions: `CLAUDE_BIN=$(readlink -f
~/.local/bin/claude) CODEX_BIN=$(readlink -f ~/.local/bin/codex) cargo
test -p olo-token-pool --test clients -- --ignored --nocapture`
(`CODEX_LOG` for Codex's own log). Run it when either client updates:
Codex 0.166 asks `/backend-api/wham/accounts/check` before a turn (the
fake answers as ChatGPT does), and both must come out as their own
programs to `peer`. `codex_daemon_follows_the_pool` starts Codex's
daemon on a proxy that goes nowhere, changes `.env`, and checks the pool
restarts it, sees it busy in the middle of a turn, and that the new one's
turn goes through the pool. For the window, start a session with a fixture `--home`
holding fixture credentials, a fake upstream
(`OLO_POOL_UPSTREAM=http://127.0.0.1:PORT`: its `/oauth` takes
refreshes), `OLO_POOL_PORT` and `OLO_POOL_KEYFILE`, and a fake
`claude` or `codex` on `PATH` for separate sign-ins. Never real accounts.

## Telegram (apps/telegram, crates/telegram)

A Telegram client of Olo's own: MTProto in process through grammers
(`olo-tg`), no Telegram Desktop and no tdlib.

* **olo-tg** (`crates/telegram`): `model` is chats, messages, topics,
  folders and contacts as plain data; `backend` the commands the app
  sends (`Cmd`) and the events it hears (`Event`) over channels, from
  `net` (the real account, on a tokio runtime of its own thread: raw TL
  for the dialogs, history, replies, forum topics, folders, search,
  read state, drafts, pins and archive, notify settings and typing;
  grammers' client for sending, editing, deleting, forwarding,
  reacting, pinning, uploads and downloads; updates converted as they
  come) or `demo` (a pretend account answering as the real one would,
  with no network: people, groups, a channel, a forum, folders and the
  archive, every kind of message; `OLO_TELEGRAM_DEMO=N` adds N more
  chats, `OLO_TELEGRAM_DEMO_EVERY=S` a message every S seconds).
  `account` keeps the API ID and hash and the session's secrets (home
  datacenter, authorization keys) in the login keyring and the rest
  (peers' access hashes, update state, cached chats, messages and media)
  in a folder only the person can read; signing out clears both.
  `OLO_TELEGRAM_SECRETS=DIR` and `OLO_TELEGRAM_CACHE=DIR` keep them in
  folders instead, so tests never touch the keyring. `signin`: the
  phone code, the QR code (`auth.exportLoginToken`) and the two-step
  password.
* **Setting it up** is in Preferences ▸ Integrations ▸ Telegram: the
  API ID and hash the person registers at my.telegram.org (never
  Telegram Desktop's), then signing in by phone or QR, the password if
  asked; Sign Out. The app is told (`--reconnect`) and picks the
  account up.
* **The app** is on the GPUI kit (`olo-ui`). `store` is what is known,
  kept up to date from the events (the list's order, each thread's
  loaded messages with a version bumped on change, our messages still
  sending, who's typing); `pics` asks for pictures the first time they
  are drawn and decodes them on two workers into GPUI images (BGRA).
  `list` is the chat list (the search field, asking Telegram after a
  pause in typing; folders as chips with the rest under More, pinned
  chats, the archive's row and Unknown Senders', unread and mention
  counts, typing in place of the preview; while searching, matching
  chats then Telegram's people, chats and messages): its rows are
  painted in one canvas, only those near the view, and found under the
  pointer by arithmetic (60 pt each, the picture 40 across). The list
  comes as the main list (folder 0) then the archive (folder 1), each
  chat's place as Telegram has it; once it's whole, chats it no longer
  gave go (`Store::whole`: left, deleted, cached from before). A page
  that fails is asked again (`dialogs_retrying`: after the FLOOD_WAIT
  Telegram asks for, shown as Waiting, else 2, 4, 8… s; six tries), one
  load at a time, so a list never stops halfway with the rest as the
  cache had it. The cache (`chats.json`, read first for an instant list)
  follows the list as it changes, not only after a whole load: a second
  after the person's own change (a chat archived, pinned, muted,
  deleted) or a whole list, else within 30 s (messages coming in; about
  1.3 ms for 1,500 chats, on a blocking thread). Archiving or
  unarchiving takes the folder from Telegram's answer
  (`updateFolderPeers`), else asks for the chat's dialog
  (`getPeerDialogs`), and shows where Telegram has it, saying so if
  that isn't where it was asked to go. Each load ("telegram: the chat
  list: N in the main list, M archived, in P pages"), each page asked
  again and each move are logged (`olo-logs --print --program
  olo-telegram`).
  **The person's changes reach Telegram** (`outbox.rs`): archiving or
  unarchiving, pinning, muting, reading and marking unread go into an
  outbox written to disk (`outbox.json` in the cache, whole then moved
  into place) before they're sent, and leave it only when Telegram has
  them; until then the chat shows the change over whatever Telegram or
  the cache still say (`Outbox::apply_to`, in `put_chat` and on every
  page of the list). A later change to the same thing replaces one
  waiting. A task sends them: archive moves together, up to 100 to an
  `EditPeerFolders`, the rest one by one; a refusal (4xx) drops the
  change, says so and reads the chat again; a FLOOD_WAIT, an outage or
  a chat not known yet waits (Telegram's wait, else 5 s doubling to a
  minute) and goes again, at once when the connection is back or
  another change is made; a change for a chat gone from a list read
  whole is dropped. What's left goes when the app starts again: never
  lost to quitting, a crash or a kill. `Event::Sync { pending, total,
  waiting }` drives a line under the chat list ("Sending 3 changes to
  Telegram…", a bar, "2 of 5 sent", Telegram's asked wait, or offline),
  and quitting with changes on their way shows them going and quits once
  they're sent, or after 30 s, or at Quit Now (`QUIT_WAIT`; whatever's
  left is kept). (Before, each change was a request of its own that
  nothing kept: quitting dropped those in flight, and "Unarchive 40
  Chats" sent 40 at once, which Telegram throttles.) The demo account
  shows it with `OLO_TELEGRAM_DEMO_SEND_MS=1500` (each change "sent"
  that long after).
  **Updates, each applied once.** Every update in an account's sequence
  has its place in it (pts; each channel its own), and grammers fills
  gaps and drops repeats within a run but keeps where it got to only
  when asked (`UpdateStream::sync_update_state`, "not automatically
  done"): it's asked after every update handled, and the session saves
  it with the rest every 10 s and on quitting. (It never was, so every
  start replayed everything since signing in, onto the fresh list: old
  messages counted unread again, old moves to the archive undoing
  later ones.) Starting again catches up from there when that's under
  six hours old (`CATCH_UP_SECS`), else starts from now (the list is
  read whole anyway; this needs the account itself among the peers
  kept, `Store::knows_self`, else it still catches up). And what an
  update says can't undo what's newer: an incoming message counts
  unread only if newer than the row's last message and than what's read
  (`counts_unread`), a read only if it reads further (`read_moves_on`),
  and an update that changes a chat's state (archived, read, marked
  unread, muted, pinned) has the chat read again from Telegram a moment
  later, in batches of up to 100 (`confirm`, `getPeerDialogs`), what it
  has now winning.
  **Unknown Senders** (`Chat::unknown`, `model::stranger`): a person who
  isn't a contact (nor verified, nor Telegram's service notifications)
  and whom we've never written to (nothing of ours read there, the
  latest not ours) is in a list of their own, behind its row: no
  notification, no unread count, in no folder, until we write back.
  **Chosen together**: ⌘-click chooses chats one by one (with the one
  open), ⇧-click a run of them, Edit ▸ Select All Chats every one the
  list shows; the Chat menu and a chosen row's menu then act on them
  all (Archive, Unarchive, Mark as Read, Mute, Unmute, Delete…, which
  asks, saying how many; `Cmd::DeleteChat` deletes a person's history
  and leaves a group or channel, as Telegram's apps do). `conv` is the conversation: its header, a forum's
  topics as chips, the messages laid out once per message and width
  (`bubble::Laid`, kept by a digest of what the layout depends on) and
  painted in one canvas inside a `ScrollView`, only those within a
  screen of the view; it stays at the bottom while it was there, keeps
  what's in view where it was when older pages come in above, and
  finds what's under the pointer (`bubble::hit_at`: links, mentions,
  replies, pictures, reactions) from what was last painted, so painting
  needs no pointer. The composer is a multi-line `TextField` (Return
  sends, ⇧Return makes a line) under what's being answered or edited
  and the files going with it; the draft is kept per chat and saved to
  Telegram on leaving it. `bubble` lays out and paints a message:
  replies, forwards, photos and video frames, voice notes' waveforms,
  files, stickers, polls, link previews, reactions, read ticks, and
  the text's entities (code on its ground, spoilers hidden, links),
  colour emoji drawn by Olo's GPUI text in line. Both panes scroll, so
  each is a grid of its own for the layout review (`review::own_grid`),
  as are avatars and the time in a balloon's corner. A chat's Info is
  a window over its chat (`WindowOptions::child`; Escape or ⌘W puts it
  away), Forward To a dialog, Contacts a window of its own; deleting
  and signing out ask in an alert (`open_alert`). The chat, topic,
  folder and list width are kept over an upgrade (`on_upgrade`).
* **What a frame costs**: the list, the conversation and the composer
  are panes, each a view of its own drawn as a cached view
  (`AnyView::cached` with its bounds): the account's news (the model
  notifies) draws them all again, but the pointer over a row, a
  scroll, or the caret blinking in the composer draws only its own pane
  (a hover notifies just its pane's view: `cx.notify(id)`), the others
  replayed from the last frame. The conversation is planned once per
  change to its thread (each entry's top and height), so a frame does
  only what's in view; a newly opened chat is laid out newest first,
  3 ms a frame, the rest guessed until the next; the layouts of the
  last 12 chats left are kept, so going back costs nothing. Measured on
  release builds against the egui app (demo account, 2×): 96 MB
  against 133; 30 pointer moves over the messages 8 frames against
  157; ten chat switches 43 frames and 115–137 ms against 114 and
  175–193 ms; 24 notches of scrolling 204–221 frames, 1.5 ms each
  (egui's 1.2), slowest 3.6 ms; a caret blink 1.2 ms (the window's
  2.6 before the panes).
* **Around it**: new messages notify through Olo's notifications,
  answered from the banner (`Note::reply`, `NoteEvent::Replied`); the
  unread count is Deskbar's badge (`Notifier::badge`); folders, chats
  and messages are commands with keys (`app_keys::TELEGRAM`: ⌘1–8 for
  folders, ⌘9 the archive, View ▸ Unknown Senders, ⌘↑/⌘↓ the previous
  and next chat), and
  every row, bubble and button in view is in the agents' tree (a
  scrolled list's rows painted beyond its edges aren't).
* **Not yet**: recording voice notes (no Opus encoder in Rust yet), and
  animated stickers show their first frame.
