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
   device), runs on a worker thread and reports back with `cx.proxy()` /
   `App::message`. No `std::fs` on folders of unknown size, no blocking IPC
   round-trips and no `wait()` on the UI thread.
2. **Show progress and keep it live.** Long work shows what it is doing:
   counts, the current item, a `StatusBar` for work with a known size, and
   a "Searching…"/"Copying…" state otherwise. Results stream in as they
   arrive instead of appearing all at once at the end.
3. **Everything long can be cancelled** (Stop button, Escape, closing the
   window), and cancelling takes effect immediately (workers check an
   `AtomicBool`).
4. **The interface reacts in the same frame** as the input that caused it:
   pressed states, selection, menus and text never wait for work to finish.
   When an action changes state, `ctx.request_repaint()`.
5. **Batch updates** from workers (every ~100–150 ms) so streaming results
   never flood the event loop, and poll shared progress counters with
   `request_repaint_after` only while work is running.
6. **app_server never waits on a client.** Clients that stall must not stall
   the desktop. Nor on the GPU driver: a client's new buffer is checked
   (its first import, milliseconds on some GPUs, every frame of a resize)
   on a worker (`dmabuf_check`), and a commit showing it waits for that
   rather than importing it on the loop. Measure with
   `tools/dev/perf-fuzz` (`{"type":"timings"}` names what held the loop).
7. **Stay current without polling.** What is on screen reflects the system
   as it changes: follow files and folders with `olo_kit::watch` (inotify:
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
8. **app_server never deadlocks.** A hang there freezes the whole desktop.
   Smithay's per-surface lock isn't re-entrant: inside a closure given to
   `with_pending_state`, `with_states`, `with_committed_state` or
   `with_cached_state`, call nothing that takes it again
   (`WindowElement::decorated`/`inner_size`/`outer_size`/`content_offset`/
   `title`/`app_id`/`geometry`, sending a configure, mapping the element).
   Work such values out before the call. `tests/surface_locks.rs` checks
   the source for this; `tools/dev/wm-sweep` exercises every window
   operation on several screens and fails on the first hang. Use
   `std::sync::Mutex` only with nothing else locked and never across a
   call back into app_server.
9. **Every frame within 8.3 ms: 120 frames a second, always.** Olo's
   displays run at 120 Hz, and a frame that takes longer than one
   skips. That covers a whole frame (egui, the app's code, tessellating
   and the GPU) on release builds, filling a 16" MacBook Pro's screen,
   at 1×, 1.5× and 2×. Nothing per frame touches the disk or waits on
   anyone (an icon is loaded off the thread; a desktop entry looked up
   once, on a worker); what a frame lays out is what shows, not all
   there is (History's cards, lists' rows); text laid out once is
   egui's cache's, so don't make new strings every frame for what
   didn't change. Nor does it set the GPU up: configuring a swapchain
   waits for the GPU to go idle (3 ms, 30 with a busy GPU), and a new
   egui context lays out its glyphs and uploads its font atlas (6 ms).
   So the kit gives a window being resized a swapchain with room to
   grow, showing only its part (`wp_viewport`), made to fit 1.5 s
   after only if it has a quarter more room than it shows (`roomier`:
   a theme switch that changes a window's size a little sets no
   swapchain up), and a menu takes on the
   last closed menu's surface, swapchain, context and renderer
   (`SpareMenu`, two kept); the first menus' are made ahead, the
   contexts laid out on a worker as a window first draws, and so no
   menu waits, the first included. Nor does a frame wait on another
   thread holding a lock for long: wgpu's queue lock is held through
   `queue.write_texture`'s copy, and its allocator's lock while the
   driver allocates (17 ms for 100 MB), so a worker uploads through a
   buffer of its own a band of rows at a time (`upload_texture`), and
   wgpu-hal is patched to give big textures their memory outside the
   lock (vendor/README.md). A texture of 4 MB and up is uploaded on a
   worker (`olo_kit::textures`: `uploading(ctx, id)` says it isn't in
   yet; show something smaller meanwhile, as olo-preview shows the
   image's next smaller level). A frame's shapes are tessellated on
   several threads when there are many (`tessellate`), and text keeps
   its shaping across wrap widths and colours (vendored epaint's
   `ShapedCache`; sections differing only in colour are shaped
   together, with a shape plan made once per face), so a live resize
   or a theme switch only breaks lines again. `tools/dev/bench` checks
   every app (see Testing).

   A frame's budget is the frame less what the GPU then takes to draw
   it: app_server shows a commit once its GPU work is done (the dmabuf
   blocker), so a window drawing in 6 ms whose GPU took 2 more missed
   every other frame. app_server tells clients to draw as it starts
   drawing the screen, not once it has (`send_frames` in `pre_repaint`),
   so they have the whole frame. On Honeykrisp new GPU memory costs
   about 0.35 ms a MB, so egui's vertex and index buffers grow on a
   worker before they fill (vendored egui-wgpu, `SlicedBuffer::reserve`)
   and wgpu-hal gives big buffers their memory outside its lock.

   What looks the same frame after frame isn't made again:
   * `olo_kit::recorded::Recorded` keeps the shapes a painting added
     and adds them again moved (History's cards, scrolled: 200 painted
     again took 1.6 ms a frame). The caller says when it's stale (its
     look, `icons::loaded()` for an icon that was loading) and paints
     afresh while AccessKit is on (only painting describes its text).
   * `Cell::text_cached(key, || job)` keeps a list cell's galley while
     its key (what the text is made from), width, colour and selection
     stay the same (Logs' rows: 2 ms a frame).
   * Anything that keeps galleys across frames checks
     `ctx.fonts(|f| f.galley_generation())`: when it changes, epaint
     emptied its glyph atlas (it filled up) and old galleys paint the
     wrong glyphs.
   * Work a frame can't do in its budget is spread over frames: History
     measures cards again at a new width for 1 ms a frame (the rest
     show as they were a frame or two longer); Logs shapes the rows a
     new filter will show while it finishes (epaint keeps shaped text a
     while), so the frame that shows them only breaks them into rows.
   * Text hundreds of pixels high (a screen saver's words) goes through
     `olo_kit::big_text`: a worker with fonts of its own rasterises it,
     each glyph a texture of its own, and it shows a frame or two later;
     in egui's atlas each glyph was rasterised on the UI thread and the
     atlas sent to the GPU again whole as it grew.

## Layout

| Path | What |
|---|---|
| `crates/app_server` | The compositor (Smithay, forked from Anvil). Draws the BeOS window decorator (yellow tabs), manages windows/workspaces, serves IPC. |
| `crates/session` | `olo-session`: starts and supervises app_server, Deskbar and the desktop; in-place reload and upgrade (see below). |
| `crates/protocol` | JSON-lines IPC between app_server and our apps (`Client`, `Request`, `Reply`, `Event`, `Settings`). |
| `crates/theme` | `Palette` (light/dark), BeOS tint helpers, decorator metrics, font lists. |
| `crates/kit` | The Interface Kit: Wayland client platform, GPU rendering (egui on wgpu/Vulkan), menus, BeOS widgets, icons, `.desktop` entries. **All apps are built on it.** |
| `crates/xdg` | `olo-xdg`: freedesktop plumbing shared by app_server and the kit (re-exported there): data dirs, `.desktop` entries, icon lookup/decoding (`load_icon`, in the current theme: `themes`), our own apps (`apps::our_app`), MIME types (`mime::for_path`), opening files and URLs with the right app (`open::open`, `open::handler_cached`: never `xdg-open`; Olo's apps first, except that links and web pages go to the browser chosen in mimeapps.list, else `olo-chrome`; and the same for other programs: `open::for_others` gives the hidden `olo.viewer`, `olo.styled-edit` and `olo.tracker` desktop entries, each listing every type shared-mime-info knows that Olo opens with it, and an `olo-mimeapps.list` making them the defaults, which GIO, `gio open`/xdg-open and xdg-desktop-portal's OpenURI read before `mimeapps.list` while `XDG_CURRENT_DESKTOP` names Olo; olo-session writes them into `~/.local/share/applications` at start, makes that folder's `mimeinfo.cache` again when they change (`open::mime_cache`, as update-desktop-database would: GIO only finds an app for a type through it) and removes ones it wrote that are no longer wanted, `X-Olo-Written=true`; and, once Olo's Chrome is installed, `open::offer_browser` writes `~/.local/share/applications/olo-mimeapps.list` naming it for web pages and links, read after every `mimeapps.list` in the config directories so a browser the person chose wins, but ahead of the system's lists), `find_program`, `spawn_detached` (`spawn_detached_app` and `open::spawn_app` for a desktop entry's program, in a scope of its own: `scope`). No egui. |
| `crates/system` | `olo-system`: the system over its own protocols, in process: logind (`power`: reboot, power off, suspend, inhibitor locks), NetworkManager (`network`: status, join including 802.1X, forget), the sound server (`audio`: volume, mute, `watch` change feed), dconf writes, the D-Bus/systemd activation environment. All blocking: call from worker threads. |
| `crates/pool` | `olo-token-pool`: token pooling for Claude Code and Codex (see "Token Pool"): the accounts, their sealed store, the drain order, the loopback gateway and the clients' proxy (its own certificate authority, and which program is connecting, from /proc). |
| `crates/ai` | `olo-ai`: how AI features reach their models (see "AI features"): `ask(&settings.ai, &feature, &Ask)` → the answer's JSON. Blocking; call off the UI thread. |
| `crates/preview` | `olo-preview`: native previews of every file kind (text, images, PDF, audio, video, archives, office documents, fonts). Source code is coloured by syntect's bundled Sublime grammars plus Olo's own in `src/syntaxes` (TOML, with Cargo.lock and other lock files; INI, with systemd units, .desktop entries and git and editor configs), its scopes mapped onto the theme's token classes (`text::classify`). Spreadsheets and CSV (`table.rs`, sheets shared as `Arc`s): every column, scrolled sideways; the first row as the columns' names when it looks like a header (all text, with numbers or blanks below); number columns lined up on the right; a bar with Search (every word, in all columns or the one chosen beside it; Escape clears, then lets go) and how many rows show ("14 of 120 rows"); a header click sorts (again to reverse): numbers as numbers (a thousands comma, a currency sign or a percent allowed) before text, text naturally and without case ("file2" before "file10"), empty cells last, ties in the file's order. A sheet of more than 40,000 cells is searched and sorted on a worker (cancelled when asked again), the rows shown until then staying put and the bar saying "Searching…". Quick Look's own keys (Space, the arrows, F) stand aside while a field has the keyboard. Formatted documents (Markdown, and Word, OpenDocument and PowerPoint text) are blocks (`doc::Block`) set as a page by `rich.rs`: Markdown is CommonMark with GitHub's tables (their alignment, and styled cells), task lists, strikethrough and alerts (`> [!NOTE]` and the rest, a bar and title in the alert's hue), typographer's quotes and dashes; raw HTML keeps its text (`<br>` breaks, `<img>` is an image, comments go); code blocks are coloured by their fence's language (`text::highlight`, the text preview's grammars); pictures beside the document are shown (local files only, never fetched; a remote one's alt text shows instead), a texture pixel to a device pixel when they fit and from the halving that fits when they don't. The page is at most 680 points wide, centred, body text 13 points on a 20-point line, headings 26 to 13 points with room above them (rules under the first two), lists with hanging indents (bullets drawn, numbers lined up on the right, tasks as `check_box_look`s), inline code on a tint with a small no-break space either side (a line doesn't break between "(" and code), and links in the link hue, underlined under the pointer; they open with `olo_kit::open` (a relative one beside the document) and a `#heading` scrolls to it (GitHub's anchors). Blocks are laid out once for a width, scale, look and font generation: those in view first (at most 5 ms), the rest 3 ms a frame after, and a spinner shows over a part not laid out yet; only blocks in view are drawn. Agents read each block (`Heading`, `Paragraph`, `ListItem`, `Code`, `Table`, `Image`) and click its links (`Link`). Used by Tracker's Quick Look (`Preview::new`, images capped at 3200 px) and the Viewer (`Preview::new_zoomable`: full resolution up to the GPU texture limit, zoom and pan, `zoom` / `set_image_scale`). Images carry halved copies (`load::halvings`) because egui-wgpu makes no mipmaps. Video (`video.rs`, H.264 in mp4 or Matroska and AV1 in Matroska/WebM, with `media.playable` set): a worker demuxes and decodes (OpenH264, or rav1d through `olo_images::av1::Decoder`) up to six frames ahead, giving each picture the earliest outstanding presentation time (OpenH264 returns display order; it must be made with `Flush::NoFlush`, as the crate's default flush after every packet breaks B-frame reordering and every B-frame then fails), mp4 times shifted by the track's edit list; `Video::tick` shows frames against a clock that follows the sound's reported position when an `audio::Player` is playing it and runs alone otherwise (no sound server in tests, silent files). Seeks go to the key frame before the target (Matroska: scans back for one, further each time) and drop pictures before it; a paused seek still shows its frame. `OLO_SAMPLES=dir cargo test --release -p olo-preview video -- --ignored` checks seeking on test.mp4/mkv (one key frame) and gop.mp4/mkv (one a second), made with ffmpeg's testsrc2, plus av1.webm (svt-av1, Opus sound) and bframes.mp4 (any x264-made H.264 High clip with B-frames; `b_frames` checks every picture comes out, in order); `keeps_up` needs av1-1080.mkv (1080p AV1) to decode at well over 30 frames a second (72 on the M2 Max; the YUV to RGB fast path is fixed-point with tables, checked against the float path by `yuv_fast_path`). Audio decodes with symphonia plus `opus.rs` (opus-decoder, OpusHead parsing and pre-skip trim) for Ogg Opus. Picture editing (`edit.rs`): an `Editor` keeps a list of `Op`s (rotate, flip, crop, resize) with undo/redo and a saved mark, and a worker that loads the full picture once, applies new steps to its last result, sends `EditEvent::Shown` (downscaled for the screen, with halvings) for `Preview::show_edited`, and writes PNG, JPEG (quality 92), WebP, BMP, TIFF or GIF atomically (`write`, through a hidden `.name.saving` file); `edit::editable` says which files it can write back. `CropTool` is the crop overlay (drag a frame, move it or its edges; `Preview::image_rect` gives where the picture is drawn, `set_still` stops pan and zoom while it is up). The Viewer's Image menu and File ▸ Save / Save As / Revert use it; Quick Look's footer has Rotate Left / Right (⌘L / ⌘R), which save at once. |
| `apps/<name>` | One crate per application, binary named `olo-<name>`. Picked up by the workspace glob. |
| `vendor/` | Patched copies of dependencies (`[patch.crates-io]`), each explained in `vendor/README.md`; drop them when upstream has the fix. |
| `data/icons` | Icon set, pre-scaled to device sizes (`data/icons/<px>/<name>.png`). |
| `data/themes` | The other looks of the icons and wordmark (`data/themes/<id>/icons/<px>/<name>.png`, `brand/`), chosen in Preferences ▸ Themes; see "Themes" in [ICONS.md](ICONS.md). |
| `data/session` | `olo.desktop` for the login screen (installed by `tools/install-session.sh`). |
| `tools/` | `install-session.sh` (release build → `~/.local`, session entry → `/usr/local/share/wayland-sessions`), `dev/tsession` (headless UI testing), `genicon.py`/`genicons.sh`/`mkicons.py`/`iconsheet.py` (icons, see [ICONS.md](ICONS.md)). |

Build: `cargo build -p olo-<app>` (plus `cargo build --bin olo-app-server` once, for headless testing; as root set `CARGO_HOME=/home/ben/.cargo` to share the registry) (repo is owned by `ben`; when running as
root use `sudo -u ben bash -lc 'cd … && cargo build …'` in the main checkout).
Builds go through [kache](https://github.com/kunobi-ninja/kache), a
compiler cache (`cargo install kache --locked`, then
`kache init --no-shell --no-service`, which sets `build.rustc-wrapper` in
`~/.cargo/config.toml`): a crate compiled once with the same inputs is
restored instead of rebuilt, in any target directory or worktree. It is
ben's setup, so build as ben (as above); `kache doctor` checks it,
`kache stats --last-build` shows the hits, `KACHE_DISABLED=1` bypasses it.
Builds are kept lean on memory and disk. Dev builds keep only line tables (file and
line in panics and backtraces), and dependencies keep no debuginfo, so debug binaries
stay small and cheap to link. Full debuginfo made each one 600–850 MB, and twelve
links at once ran a 30 GiB laptop out of memory. On ben's machine,
`~/.cargo/config.toml` also caps `build.jobs` at 6 and links with lld
(`-C link-arg=-fuse-ld=lld`). `~/.config/kache/config.toml` caps the cache at
15 GiB and has kache prune units no build used for 3 days and target dirs idle
for 14 days; otherwise a target dir only grows (olo's reached 439 GiB).

## Writing an app

```rust
use olo_kit::{App, Cx, Menu, WindowId, WindowOptions, egui, widgets::*, metrics as m, style};

struct MyApp { /* state */ }

impl App for MyApp {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut egui::Ui) {
        window_content(ui, |ui| {
            ui.add(TextControl::new(&mut self.name).label("Name:"));
            if ui.add(Button::new("OK").default(true)).clicked() { /* … */ }
        });
    }
    fn menu_bar(&mut self, _w: WindowId) -> Option<Vec<Menu>> {
        Some(vec![Menu::new("File").item("Quit", "quit", Some(egui::Key::Q))])
    }
    fn command(&mut self, cx: &mut Cx, w: WindowId, command: &str) {
        if command == "quit" { cx.quit() }
    }
}

fn main() {
    olo_kit::run("olo.myapp", |cx| {        // app id: olo.<name>
        cx.open_window(WindowOptions::new("My App", 480, 320).min_size(320, 200));
        MyApp { /* … */ }
    });
}
```

* **`App`**: `ui` (draw a window below its menu bar), `menu_bar`, `command`
  (menu item or ⌘-shortcut), `close_requested`, `message` (from background
  threads via `cx.proxy().send(msg)`), `theme_changed`, `dropped` and
  `drag_ended` (see "Drag and drop"), `save_state` (see "In-place
  reload").
* **`Cx`**: `open_window`, `close_window`, `set_title` (call it as the
  title changes, not only while drawing: a window whose content isn't
  shown, behind another in its tab group or covered, is let draw once a
  second, yet its tab may show; Terminal's `retitle`), `resize`, `redraw`,
  `activate` (bring a window forward and focus it, restoring it if it is
  minimized or on another workspace — via xdg-activation; use it when a
  forwarded launch asks for a window that is already open),
  `on_this_workspace(id)` (the window is on the workspace the active
  display shows, as app_server last said; see Single instance), `set_path(id, path)` (the
  file or folder the window shows, also `WindowOptions::path`: its tab's
  menu then offers Open in Tracker, and its tree carries it as `path`),
  `copy` (text
  to the clipboard), `copy_secret(text, clear_after)` (marks the offer
  `x-kde-passwordManagerHint: secret` so clipboard histories skip it, and
  withdraws it after `clear_after` if it is still ours), `copy_data` (any
  MIME types, e.g. `image/png`; the data is written to pasters from a thread), `paste` (reads the clipboard
  off-thread and delivers an `egui::Event::Paste`), `paste_data(window,
  types)` (the clipboard in the first of `types` it has, a type ending in
  `/` standing for that kind, read off-thread and delivered as a
  `Pasted { window, data: Option<(mime, bytes)> }` message: for files and
  pictures), `quit`, `stay_resident` (keep running with no windows), `palette`,
  `proxy`, `window_size`, `restored_state`, `activation_token(window,
  tag)` (an xdg-activation token from the latest input in the window,
  delivered as an `ActivationToken { tag, token }` message, to hand to
  another program so its window may come forward; app_server honours a
  request only with fresh input from the asking app, a click handed on
  by one of our programs, or a token it issued itself:
  `olo_protocol::launch_activation_token` asks for one
  (`Request::ActivationToken`) for launchers run without a click, as
  olo-chrome does so a link opened from a shell or an agent brings
  Chrome's window forward from whatever workspace), `set_keyboard(window,
  Keyboard)` (a layer window taking the keyboard or not). Worker threads may call
  `ctx.request_repaint()` (or `_after`) on a window's context: the kit
  wakes its loop through egui's repaint callback and redraws that window.
  There is no clipboard manager,
  so an app that quits (or closes its last window) while it holds the
  clipboard stays running, windowless, until something else is copied.
  Its windows are gone then: a later launch (forwarded to it) must open
  a new one, so check `cx.is_open(id)` before activating a window you
  kept.
* **`olo_kit::clipboard`**: the clipboard for a program with no window
  and no kit loop (a shortcut's screenshot). `hold(clip)` puts it on the
  clipboard and serves it until something else is copied, then returns
  (so the program stays running until then); `text(s)` is a `Clip` in the
  forms text pastes as (what `Cx::copy` offers). It goes through
  `ext_data_control_v1`: `wl_data_device` only lets the client with the
  keyboard set the clipboard, and a windowless one never has it. For
  files, as file managers copy them (and Chrome, Electron, GTK and Qt
  paste them): `files(paths, cut)` is a `Clip` of
  `x-special/gnome-copied-files` ("copy" or "cut", then the URIs), KDE's
  cut mark, `text/uri-list` and the paths as text, for `Cx::copy_data`;
  `copied_files(mime, data)` reads one of `FILES` back (paths, and
  whether they were cut). `offers(types)` says whether the clipboard has
  any of them, as the app last heard (its windows hear while they have
  the keyboard): for enabling Paste. Tracker copies files so, and its
  Paste takes files (moving cut ones, then holding them copied where they
  went), a picture (`Pasted Picture.png`, of its kind) or text (a
  clipping).
* **Single instance** (`olo_kit::instance`): call `instance::forward(app_id,
  &args)` first in `main` (returns true if a running copy took the
  arguments; exit then), and `instance::listen(app_id, cx.proxy())` in the
  init closure; forwarded launches arrive in `App::message` as an
  `instance::Launch { args, cwd }`. The socket is named for the display
  (`olo_xdg::display_name()`: `wayland-1` also when `WAYLAND_DISPLAY`
  names the socket by its full path; use it for any file kept per
  display).
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
  (`cx.on_this_workspace(id)`; one on another display is elsewhere),
  and otherwise opens a new one here, rather than activate one
  elsewhere, which would go to its workspace or display. (Tracker's folders and StyledEdit's
  bare launch do so, and Terminal's `--tab` goes to a window here; a
  document already open elsewhere is still brought forward there, as two
  windows on one file would disagree.) The kit knows where its windows
  are from app_server's events: `WindowInfo::surface` names each window's
  `wl_surface` as its program knows it.
* **Open in Tracker** (`olo_kit::tracker`): wherever something has a
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
  open their file in Tracker.
  A window's yellow tab offers it too, for a window that says what it
  shows (`cx.set_path`: StyledEdit's document, the Viewer's file,
  Terminal's working folder; Tracker's own windows don't need it).
  Tracker's Find results have it as Open Parent (⌘↑, also in their
  context menu, with Open and Quick Look). Don't write another; the
  label and the command are the same everywhere, so agents find it by
  either.
* **Following a setting** (`olo_kit::settings`): `current()` reads
  app_server's `Settings` once (a quick local round trip; fine at
  startup), `watch(f)` calls `f` on a thread now, after every
  `SettingsChanged` and after app_server restarts; send what you need
  through `cx.proxy()`. Tracker follows `tracker.default_view` (the view
  new folder windows open in, column view by default) this way.
  `update(edit)` changes them from an app (read, `edit`, `SetSettings`, on
  a thread), and `watch_power(f)` follows `PowerStatus` the same way
  (the screen locking, the displays sleeping).
* **Menus for the command palette** (`olo_kit::commands`, automatic):
  every kit app answers on `$XDG_RUNTIME_DIR/olo-menus-<pid>.sock`
  with the menu bar of its last active window, flattened
  (`commands::menus(pid)`: path, label, command, keys as shown, enabled,
  checked), and chooses an enabled one as if from the menu
  (`commands::invoke(pid, command)`), logging the command and who asked
  (`asked to choose a menu command`, the sender's name and pid). Both
  block for at most a second; call them off the UI thread.
* **What a window shows, for agents** (`commands::tree(pid)`, automatic):
  `{"tree":true}` on the same socket turns on egui's AccessKit output for
  one frame of every window and answers with a `WindowTree` per window
  (title, content size, active, nodes in tree order: role, name, value,
  bounds in the content's logical pixels, enabled, clickable, focused,
  checked, selected); it then turns AccessKit off again, so nothing is
  spent on it otherwise. A program that isn't a kit app can answer for
  another process with `commands::serve_trees(pid, app_id, trees)` (the
  Olo extension's host does it for Chrome). Layer surfaces (Deskbar,
  the desktop) answer too, marked with their namespace (`layer`).
  `{"hints":[ID…]}` asks only about the surfaces named (their
  `wl_surface` protocol ids, which app_server knows them by) and answers
  as soon as those have drawn, each tree marked with its `surface` and
  cut by `WindowTree::pickable` to what takes clicks and what holds it
  (a window's text, thousands of nodes, goes): keyboard hints ask this
  way, so they never wait on a window nobody can see. (`serve_trees`
  can't tell its surfaces apart and answers it with all of them.)
  `{"path":ID}` (`commands::path(pid, surface)`) answers with the file
  or folder that surface's window shows (`Cx::set_path`), or null.
  egui gives its own widgets roles and names; text
  painted by hand is only pixels, so `style::label` and `Cell` describe
  theirs with `style::describe_text(ctx, role, text, rect, selected)`.
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
  from taking input to handing the frame to the GPU), and the wgpu
  adapter drawing them (a `Cpu` one would be software rendering). The
  UI thread answers it, so how long the answer takes is how long the app
  would keep a person waiting (`tools/dev/perf-fuzz`).
  **Do the same for any text you paint yourself** that an agent should be
  able to read or click; a painted item that takes clicks (an icon, a
  column's row) is `style::describe_item(ctx, text, rect, selected)`, a
  clickable `ListItem`, so agents and keyboard hints can pick it.
  `{"focus":ID}` (`commands::focus(pid, id)`) gives the keyboard to the
  widget with that tree id, as AccessKit's Focus action. An area that takes clicks but has no role of its
  own (`ui.interact`) is reported as `Clickable`, named by the texts whose
  middles lie inside it (each text names the smallest such area). Better,
  give it a role and name yourself, as DeskCalc's keys do:
  `resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "7"))`.
  Blocks for up to two seconds.
* **⌘A in text fields** selects all of the field: the kit doesn't send it
  to the menu's Select All while a text field has the keyboard; ⌘C/⌘X/⌘V
  likewise stay with a text field and otherwise go to the menu's Copy, Cut
  and Paste. "Has the keyboard" is `ctx.text_edit_focused()`: egui's
  `egui_wants_keyboard_input()` is true for any focused widget (a list
  view, a button), so don't use it to mean "typing".
* **Fixed-size windows**: `WindowOptions::new(..).min_size(w, h).max_size(w, h)`
  (preference panels, alerts).
* **Dialogs** (`WindowOptions::dialog()`, with `.parent(id)`): a window
  that belongs to another and should stay in front of it without being
  its size, such as Tracker's Quick Look. The kit marks it with
  `xdg_dialog_v1` (alerts are also `set_modal`); app_server opens it over
  its parent before its first frame (kept on screen), never groups it
  into the parent's tabs and keeps any child above its parent when the
  parent is clicked, focused or raised (`wm.rs` `keep_children_above`).
  A service's dialog for another program (the screen-sharing picker)
  takes `.foreign_parent(handle)`, the `wayland:HANDLE` that program
  exported (xdg-foreign; a portal is given it as the parent window): the
  kit imports it and sets it as the window's parent, and app_server opens
  it over that window as it does over one of the app's own.
* **Raw keys** (`WindowOptions::raw_keys()`): for windows that consume the
  keyboard themselves (Terminal). ⌘-letter combinations reach the app as
  key events instead of menu shortcuts or clipboard events (⌘⇧C/X/V still
  become Copy/Cut/Paste), and Alt+text is delivered as text. With a
  command key app_server turns into Ctrl (⌘, or BeOS's Alt: the keyboard
  profile's, followed live by `keyboard::command_key`), app_server sends it to a terminal as its
  *right* Ctrl plus Shift and every physical Ctrl as the left one; the
  kit watches Control_R, and only then is a key's
  `egui::Modifiers::command` set, so `command` means ⌘ and `ctrl` any
  Ctrl. Only `command` chords look for menu shortcuts, and the window's
  menus show their Ctrl+Shift shortcuts as the ⌘ chord pressed ("⌘T",
  not "⇧⌘T", `Menu::ctrl_shift_as_command`), in the bar, context menus
  and the list agents read. Outside Olo, or with ⌘ off, `command` is
  any Ctrl, as before. egui's keyboard navigation is off in them: Tab,
  ⇧Tab and the arrows never move focus onto the window's controls (its
  menu titles, a tab strip), where the next Return or Space would press
  one and switch tabs under someone typing; a text field keeps focus it
  is given (`platform.rs`, `keep_focus_off_controls`).
* Background work goes on threads; post results with `Proxy::send` and
  handle them in `App::message` (then `cx.redraw(window)`).
* Periodic refresh (clocks, graphs): `ui.ctx().request_repaint_after(duration)`.
  Never use it to look for changes on disk; watch them (rule 7 above).
* Layer-shell surfaces (Deskbar, desktop): `WindowKind::Layer(LayerOptions{…})`;
  `LayerOptions::output` puts one on a named output (`cx.outputs()` lists
  them; the screen saver opens one per output). `LayerOptions::keyboard`
  is `Keyboard::None`, `OnDemand` (focus on click: the desktop) or
  `Exclusive` (every key while it is up: pickers such as Screenshot's
  region overlay). A click on a top or overlay layer that is `None`
  (Deskbar) leaves the keyboard where it was, never going through to
  what is under it (the desktop took it, so a click on Deskbar left
  the window being typed into without it); a menu of its closing hands
  the keyboard back to what had it as the menu opened.
* `ui.ctx().set_cursor_icon(CursorIcon::None)` hides the pointer over a
  window (screen savers). Other `CursorIcon`s go to app_server by name
  over cursor-shape-v1 (sctk does it), and app_server draws its own
  cursor for them; apps never ship cursor images.
* Staying awake: `cx.keep_awake(window, true)` holds a zwp_idle_inhibit
  inhibitor on the window (no dimming, screen saver, lock or sleep while it
  is visible) until `false` or the window closes; app_server counts these
  in `PowerStatus::app_inhibitors`. The Viewer and Quick Look hold one
  while `Preview::playing_video()`.
* Fullscreen: `cx.set_fullscreen(window, on)` fills the screen with a
  toplevel (xdg_toplevel fullscreen: no tab or border, nothing else drawn
  on that screen) or puts it back at its own size; `cx.is_fullscreen`
  says which. A `resize` while fullscreen becomes the size it comes back
  to. Quick Look toggles it with F.
* Showing all of an app's documents: `cx.tile_my_windows()` asks
  app_server (`Request::TileApp { app_id }`, off the UI thread) to tile
  the app's resizable windows on this workspace in a grid on the active
  display, in front (alerts and other fixed-size panels stay put).
* Taps: a touchpad tap brings a press and its release together; the kit
  holds such a release over to the next frame (`input::split_frame`), so
  egui sees the button down for a frame and press-and-hold controls
  (menus, scroll arrows, drags) work from taps too. The end of a scroll
  that moved in the same frame waits likewise (egui would drop the move).
* Frame time: egui's time is when a frame will be shown, not when it is
  drawn: the compositor's next frame after now, on a grid locked to its
  frame callbacks (smoothed, a frame of the output's refresh rate apart;
  `predicted_dt` is that frame). A frame drawn at once after a rest and
  the next drawn when the compositor asks are a whole frame apart on the
  screen, so animations starting from rest don't lurch.
* Scrolling: a touchpad scroll reaches egui as a phase of its own, from
  the first move to the stop (`TouchPhase::Start`…`End`, egui then moves
  exactly as far, with no smoothing), and the glide after it as another
  (`MouseWheelSource::Momentum`, from app_server's `continuous` scrolls).
  Wheel notches are eased in by the kit (`ease_wheel`): what is left
  shrinks by the time shown, 90% in a tenth of a second, and ends in a
  step of under a tenth of a point (egui's own ease ends with a jump of
  up to a point). Widgets that read raw `MouseWheel` events (Terminal,
  menus) get the eased steps; keep any rounding to lines or notches as a
  running remainder.
* The menu bar opens a menu on press, as on BeOS, reading the press from
  the frame's events (a touchpad tap brings press and release in one
  frame); pressing the open menu's title again closes it, but a menu that
  slid open as the pointer crossed the bar stays open for that press.
* Session lock (the lock screen only): `cx.lock_session()`, then
  `SessionLock::Locked(windows)` in `App::message` (one window per output,
  the first is primary; kept up to date as outputs come and go) and
  `cx.unlock_session()`. See "The lock screen".

### Drag and drop (`olo_kit::dnd`)

Over Wayland's data device, so it works within a window, between
Olo's apps and with any other Wayland program. As in BeOS (the Be
Book's `DragAndDrop.md`), a drag carries the same data in several types
and offers actions; where it is dropped picks the type and the action.

* **Dragging out:** on an egui `drag_started` (the button still down, so
  that frame), `cx.start_drag(window, DragData)`. `DragData::files(&paths,
  Actions)` carries `text/uri-list` (file:// URIs: file managers,
  browsers, editors) and the paths as plain text (terminals);
  `DragData::text(s)`; `.with(mime, bytes)` adds a type only Olo knows,
  first; `.icon(DragIcon)`; `.from_folder(dir)` names the folder the files
  are in (`Hover::from_folder(dir)`), so Tracker refuses dropping them back
  where they are, from any app; `.as_link()` asks a folder for links
  instead of copies (`Drop::wants_link`). Viewer and Quick Look drag their file out
  from the picture when it fits (`Preview::take_drag_out`: a drag that
  doesn't pan), Quick Look also from its footer's icon, Get Info from its
  icon.
  Tracker's folders are spring-loaded: a drag resting on one for
  `view::SPRING` (1 s) opens it (`item_drop` times it; `take_spring`).
  Deskbar takes files dropped on a running application's row and opens
  them with it (`open::open_with(handler, paths)`: our apps get them all
  as arguments, a desktop entry at once with %F/%U or once each).
  Dragging a row out carries the application's program `.as_link()`
  (ours, or an entry whose command is a program alone); Tracker makes a
  link named as the application, and draws links to our programs with
  their icons (`Entry::app_icon`). `icons::drag_icon(ctx, icon, size, count,
  hotspot, DragLabel::Below(name) | Right(name) | None)` makes Tracker's
  picture: the icon (stacked for several) and the name highlighted,
  see-through, the name drawn from the glyphs egui already rasterised.
  Hit-test a drag where the button went down
  (`ui.input(|i| i.pointer.press_origin())`): by `drag_started` the
  pointer has moved on. `App::drag_ended(window, action)` follows (None:
  nothing took it).
* **Taking drops:** each frame something is dragged over a window,
  `dnd::hover(ctx)` gives a `Hover` (position, types, the source's
  actions, `from_here` / `from_us`). A part that would take it calls
  `dnd::accept(ctx, target, mime, Action)` that frame (no call refuses)
  and shows it: an item highlights itself, a view or window calls
  `dnd::outline(ui, rect)` (2 px, keyboard-navigation colour).
  `dnd::open_files(ui, target)` does all of that for a window that opens
  any file dropped on it (Viewer, StyledEdit). On release the kit reads
  the data off the UI thread and calls `App::dropped(window, Drop)`
  (`target`, `mime`, `data`, `action`, `pos`; `paths()`, `text()`).
  What other programs drag: Chrome (156, on Wayland) drags a picture out
  of a page as `text/x-moz-url`, `text/html`, text and the picture itself
  as `application/octet-stream;name="sun.png"`, with no `text/uri-list`;
  a link as `text/x-moz-url`, `text/html` and text.
  `Hover::named_file_type()` finds such a file's type and
  `dnd::named_file(mime)` its name (made safe to save under);
  `dnd::moz_url(data)` reads a link's address and text (UTF-8 from
  Chrome, UTF-16 from Firefox). Tracker takes, in order: files
  (`text/uri-list`), a file by its contents (saved under its name),
  a link (a clipping of its address named by its text: "Docs link
  clipping.txt"), text (a clipping).
* **Text** (`text_drag`): press on the selection in a text field and drag
  it out, as on BeOS; dropped back in the same field it moves, anywhere
  else it is copied (a folder makes a clipping). Text dragged over a field
  shows a caret where it would go and is put there, selected; files are
  left to the window. `TextControl` and `TextView` do it themselves; a
  field of an app's own calls `TextDrag::new(ui, id, editable, password,
  multiline)` before its `TextEdit` (making `take_drop()` with
  `text_drag::apply`, or its own editing, and showing the editor
  `.interactive(drag.interactive())`) and `drag.finish(ui, &out)` after.
  StyledEdit makes a move with `Doc::move_text`, so styles and undo
  follow. A widget starts a drag with `dnd::start(ctx, data)` (it has no
  `Cx`); drops accepted with a `dnd::WIDGET` target go to the widget,
  not to `App::dropped`.
* `Action::Ask` (a secondary-button drag, as on BeOS): the place it lands
  shows a menu (Tracker: Move Here / Copy Here / Create Link Here).
* `RUST_LOG=olo_kit=debug` traces drags (start, enter, the offer told,
  drop). Headless screenshots show the drag picture; `tsession down X Y
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

### Ad-hoc popup menus (`menu::request_popup`)

A context menu is `menu::request_popup(ctx, MenuRequest { menu, source,
anchor, selected })`. The choice goes back to the widget named by
`source` (`menu::take_chosen(ctx, source)`, as `MenuField` does); if no
widget takes it in the next frame it goes to `App::command`, as a menu
bar item's would, so an app's own context menus just use its command
names.

Every item with keys shows them, right-aligned, as the keyboard
profile's platform writes them (on a Mac in Mac order, "⌘O", "⇧⌘T",
"⌃⌘H"; on Windows "Ctrl+O"; on BeOS "Alt+O"), in context menus as in the
bar. Which keys an item has comes from the profile, by its command (see
"Keyboard profiles"): an app gives the Mac's (`Menu::item(…, Some(Key::O))`,
`item_shift`), and the kit gives each item the keys its command has in
`olo_protocol::app_keys` for the profile (`keyboard::resolve`). `request_popup` gives
each item without keys the keys its command has in the window's menu
bar (`Menu::keys_from`), so a context menu's Open shows ⌘O without the
app saying so twice; keep context menu commands the bar's. Keys that
aren't a ⌘ shortcut of the window's own (a desktop shortcut from
`olo_protocol::shortcuts`, a plain key such as Space for Quick Look)
are named with `.keys("⌃⌘H")` after the item (`Action::keys`); they are
only shown, whoever gets the keys acts on them, and shown in place of
the item's own shortcut if it has one. `Action::key_label()` is what's
shown, and agents read it as the item's `value`. A shortcut on = is
pressed with + too (`Shortcut::pressed_by`: ⇧= types +), as ⌘+ zooms in
on a Mac.

In a list of windows, `.minimized(true)` after an item marks a minimized
one: a hollow diamond where the check mark goes and a lighter label
(`style::diamond`), still enabled (choosing it brings the window back);
agents read `minimized` on its `MenuItem` node.

### Tips past the window's edge (`olo_kit::tip`)

egui's tooltip (`on_hover_text`) is drawn inside the window, so in a
window smaller than what it says it is cut at the edge. `use
olo_kit::tip::Tip;` then `response.tip(text)` shows it in a popup of its
own instead (an xdg_popup with no grab and an empty input region, so it
takes no clicks), below the widget or else above it, slid to stay on the
screen. It comes and goes as egui's does (after `tooltip_delay` at
rest, at once if another showed a moment ago, not after a click), looks
the same (`Frame::popup`'s plate, `tooltip_width`), and the text is the
widget's description for agents. Use it where the tip can be wider or
taller than the window: Deskbar's tray, Terminal's sudo window's
command. Elsewhere `on_hover_text` is fine.

### Widgets (`olo_kit::widgets`)

`StringView` (a label; `.dim(true)` for hints, `.wrap(true)` for a paragraph, `.color(c)`; a wrapped one is `CONTROL_HEIGHT` tall plus a line's height, about `LABEL_ROW`, for each line after the first, so size a fixed area for it that way), `Button` (`.default(true)` for the Enter button; `.small()` is `SMALL_BUTTON_HEIGHT` tall in the small font, for actions inside something dense: a card, a list row), `CheckBox` (`.mixed(true)` shows a dash for an "All" box over boxes that differ, and a click turns it on; agents read the value "mixed"; `check_box_look` draws one that isn't a control: a document's task, done or not), `RadioButton`,
`Button::pushed(true)` for a toolbar toggle that stays down (shaded in,
its label level with its row's; only a button held down by the pointer
moves its label, a device pixel),
`TextControl` (`.label("Name:").divider(x)`; for a column of them, or
of `MenuField`s, `widgets::divider_for(ui, &["Name:", …])` is the widest
label and the gap after it, so the wells line up and every label fits), `TextView` (`.highlight(&f)`
colours it: `f(text, font, colour)` returns a `LayoutJob` in sections, a `widgets::Highlighter`,
as Log Viewer's syntax highlighting does; `.empty_text("Select a line…")` says what goes
there while it is empty, as a list's does), `EmptyState` (a whole view with nothing it can
show: "No sound server is running." in the middle of the space left, under `.icon(name)`, the
panel's or app's own; use it rather than a lone dim `StringView` in the corner of an empty
window; `.detail(text)` says why, smaller, under it, and `.action(label)` a button that does
something about it, whose clicks the widget's response carries), `Slider`
(`.label().value_text().limits()`), `MenuField` (pop-up choice), `BBox`
(etched group box with label), `separator`, `StatusBar` (progress; `level_bar(painter, rect, fraction,
fill)` paints just its bar, for list cells),
`ThumbnailPicker` (a wrapping grid of `Thumbnail{texture, color, label}`
to choose one from, the chosen one on a `style::selection_plate`, arrow keys move; Backgrounds'
wallpapers), `CellGrid` (Pulse's LED meters: a heatmap of small lit cells in sunken
wells, `GridCell{color, mark, name, selected}` each; `.rows(n)
.cell_size(px)` fixes its height, and with more cells than fit they
shrink so more rows and columns fit the same space; `.slots(n)` shows
at least n wells, `.fill()` wells across the whole width; each cell is
clickable, named for agents, its name a tooltip, the selected one
edged with the selection's hairline; `show(ui).clicked` is the cell clicked, by index),
`ScrollView` (`show`, `show_rows` for virtualised content, `show_in(ui,
rect, |ui, viewport|)` for content you lay out yourself, `reveal`
(asked for before the view shows, it shows there that frame; from
within its content, the next), `ScrollView::scroll_to(ctx, id, offset)`
(an offset set from code, by these or the bars, never goes past where
the view could scroll last frame: egui's bounds lag a frame and it
springs back from beyond them, a rubber band whose leftover speed
drifted a list up from rows just added; the rest is set the next frame,
once egui knows the new size), `.auto_hide(true)` to drop the
bar when content fits; a drag held within 24 px of an edge scrolls it on; `SCROLL_BAR` is its width;
`ScrollView::edge_scroll(ctx, past)` is how far to scroll this frame for a selection dragged `past` a
view's edge, faster the further out, the same at any frame rate), `ScrollBar`
(`::vertical(id).show(ui, rect, &mut offset, content, view)`, a lone bar
for content the kit doesn't scroll, such as a web page), `ColorControl`
(BColorControl: grey/red/green/blue ramps and number fields for an
`&mut [u8; 3]`),
`ColumnListView` + `Column` + `ListState` (sortable, resizable,
multi-select, context clicks; columns wider together than the view
scroll sideways, the header with them in the same frame (the rows are
drawn first, the header at their offset), else the last column fills
what is left; `.reveal(row)` scrolls a row selected in code
into view, with a few rows round it but never past the list's ends
(call it when the row is new to the selection, not every frame, or the
list can't be scrolled away from it); `.draggable()` reports `ListResponse::drag_started` and the
rows on screen, `visible` / `row_at(pos)`, for drop targets; in a
multi-select list, dragging from below the rows, or from a row's other
columns when it isn't selected, draws a selection band, as in Tracker's
list view: ⇧ adds what it covers to the selection, ⌘ flips it, and it
scrolls on near the edges; on a draggable list the name column and
selected rows drag the items. Selecting is a file manager's, in
`ListState`'s methods, which Tracker's icon and column views use too:
`click(row, mods)` (alone; ⌘ adds or takes away; ⇧ everything from the
`anchor`, which stays, so another ⇧-click makes the range again from
there; ⌘⇧ that range added), `move_to(row, extend)` for the keyboard
(⇧ grows or shrinks the range from the anchor; `cursor` is where the
keyboard is, `step(d, rows)` the row `d` on from it), `select_all`,
`invert`, and `band(mods)`, a `SelectionBand` whose `over(rows)` is the
selection with a band over them. The list's keys: the arrows, Page Up
and Down, Home and End (⇧ extends), Enter activates; ⌘ and Alt with them
are left to the menus. `.type_select(&|row| name)` makes typing go to
the row whose name starts with what's typed (`widgets::type_select`,
for other views: within `TYPE_SELECT_PAUSE`, 0.9 s; else the first name
after it; one letter again and again steps through the names starting
with it; a space continues a name but starts nothing, and
`typing_to_select(ctx)` says a name is being typed, so Space isn't
Quick Look then); `.empty_text("No lines match.")` says so in an empty list; a cell
draws plain text with `text`/`text_with` or sections of their own colours
with `text_job`; `text_cached(key, || job)` keeps the galley from frame to
frame while `key`, what the text is made from, stays the same. A list is
fast at any size: a cell's text (cut to fit) is drawn under the rows'
clip, not the cell's, so a list's text is one mesh and one draw call (a
hundred rows of a dozen columns, a draw a cell, took 4.6 ms to send the
GPU), what a cell paints itself (an icon, a bar) still in its cell; a
frame lays out cells for at most `CELLS_BUDGET` (3 ms), and the rows past
that (a list's first frame, a page new at once) come the next frame,
when those before are laid out; the frame is on device pixels and a
cell's text x is its column's edge and the inset, each on a pixel, so a
header and its rows agree at 1.25×), `TabView`, `TabStrip` (a row of document tabs under
a menu or tool bar, `STRIP_HEIGHT` (30) tall, roomier than a window tab,
each tab `STRIP_ROW` (26) of it standing on the bottom edge, its icon
under the menu bar's first title: `StripTab { label, icon,
status, badge, group }` each (`badge`, a `STRIP_BADGE` (12) mark, as big
as the text, in the close box's place at the tab's right, which gives
way to the close box under the pointer, as an editor's unsaved dot
does: Terminal's pull request state; like the icon, only on a tab wide
enough); tabs share the row up to `STRIP_TAB_MAX` (220) wide,
their titles cut short with "…", the others divided by a hairline; the
selected one is a raised tab with rounded top corners that opens into
the content (`.opens_into(fill, ink)` when the content isn't the panel,
e.g. Terminal's screen, so the tab takes its colour) or, with
`.closed()`, stands on the strip's edge (content that isn't that tab's
alone); `.groups(&[StripGroup { name, color }])` and each tab's `group`
(into them; a group's tabs side by side) lead each group's tabs with a
chip, as Deskbar's workspace headings: a dot of its colour and its name
in bold, quiet ink (no pill: it fills only under the pointer), a gap
before it, and a 2-point line of the colour over them from the chip
(nothing on the tabs themselves, to clash with their icons); the chip's
text is the titles' size on the tabs' centre line, not the snapped
chip's, so it is level with their titles at every scale; with
`.group_selection(&chosen)` (a bool for each group) groups can be
chosen: a chosen chip is lit as Deskbar lights the workspace showing
(`style::lit_ground`, an edge of the selection's colour, its name in
the selected text), a click on a chip gives `StripAction::ToggleGroup(g)`
and the right button `StripAction::Group(g, pos)` (its menu), where
without it either gives `Group`; agents read a chip's `selected`;
close boxes, middle-click to close, a + button, drag along the row to
reorder; it returns a `StripAction` (`Select`, `Close`, `New`,
`Menu(i, pos)` for a right-click, `Move { from, to, group }`: `to`
among the other tabs and the group it's in there, `dropped_into`:
between two of a group's tabs that group, at a group's edge the group
of the tab the pointer is over, so it joins over its first or last tab
and leaves past them, a group's only tab keeping its group unless
dropped on another; `Group(g, pos)` for a click on a chip,
`MoveGroup { group, to }` for a chip dragged, never into another group,
`group_place`; `TearOff { tab, at }` and `TearOffGroup { group, at }`
the moment a tab or chip is pulled out of the window or `TEAR` (40)
beyond the strip, the button still held: the app makes it a window of
its own, which `WindowOptions::under_pointer()` opens with its tab under
the pointer (alone rather than in its app's window group, its top kept
on the screen) and `Cx::begin_move` moves with the pointer from its first
frame, as a browser's torn-off tab; each strip keeps its layout for
`widgets::strip_place(ctx, at)`, the place among its tabs for something
let go at `at` on it, and marks that place while one of the app's
windows is moved over it (app_server says, below); `App::window_dropped
(window, onto, place)` is a window of the app let go over another,
`place` its place in `onto`'s strip if let go on it (Terminal moves its
tabs and groups in there and closes it)); each tab is a `Tab` in the accessibility tree with its
status and group ("…, in Alpha") in its name, each chip a `Button`
"Group Alpha"; `.side()` stands the tabs
down the left of the content instead, as Deskbar's list: a column
`STRIP_SIDE_WIDTH` (180) wide by default on the panel's ground, of rows
`STRIP_SIDE_TAB` (24) tall right across it, a hairline under each (fewer
points each, on the 2-point grid, when there are too many to fit); the
one showing recessed as Deskbar's active application, the one under the
pointer tinted; each group's heading a row `STRIP_SIDE_HEADING` (20)
above its first tab, as Deskbar's workspace headings (a dot of its
colour where the tabs' icons are, its name in small bold, quiet ink
where their titles are), and a 2-point bar of its colour down the left
beside its heading and tabs; the tabs' icons under the menu bar's first
title; the first row centred on a band `STRIP_SIDE_HEAD` (the menu
bar's height) beside the strip, Terminal's pane heads, and every row's
middle on the device grid (`style::snap_coord`), as the heads' are, so
they are level at 1.25× and 1.75× too; a New Tab row after the last,
and a grip along the right edge (a `Splitter`, "Tab Width")
that returns `Resize { width, done }` between `STRIP_SIDE_MIN` and
`STRIP_SIDE_MAX` for the app to keep, `done` once the drag ends; drag
up and down to reorder), `color_well`, `heading`,
`form_label`, `window_content`, `button_row`, `context_menu`,
`popup_menu_at` / `popup_result`, `InlineEdit` (Tracker-style rename in
place: returns `Editing` / `Commit` / `Cancel`; pads by `INLINE_PAD`, which
matches `style::text_highlight`), `alert(ui, icon, message, &buttons)` (a
BAlert; open its window with `WindowOptions::alert(title, message)`, or
`WindowOptions::alert_buttons(title, message, &buttons)` when its labels
are long, which widens it to fit them (`alert_width`, measured from the
font as `alert_height_in` measures the text); the last button is the
default, Escape picks the first), `button_width(ctx, label)` (how wide a
`Button` makes itself) and `button_flow(ctx, origin, width, &labels)`
(buttons' rects left to right, onto further rows when they don't fit,
and the height they take: a card's actions in a narrow window). A password alert
that talks to PAM (the polkit agent, Terminal's sudo) keeps a
`widgets::Conversation` (PAM's `prompt` and whether it echoes, the
`answer`, PAM's latest `info`, an `error`, `checking` while an answer is
with PAM; `ask`, `sent`, `ready`, `can_submit`, and `clear_answer`,
which zeroes the answer's bytes) and shows it with
`widgets::conversation(ui, &mut conv, divider, width)`: the answer field
labelled with the prompt, focused while PAM waits, and under it
Checking…, the error or PAM's message; true when entered with Return.

Cards of different sizes (`olo_kit::masonry`, the history's Cards
view): `masonry::columns(width, min, gap)` fits columns of at least
`min`; each item gives (span, height at that width) and
`masonry::arrange(cols, column, gap, &items)` places them in order, each
where the columns are lowest, filling the hole a wide one would leave
under its shorter column with the next small items that fit (looking
four ahead, so the order hardly changes); it returns their rects and the
height of the lot. `masonry::neighbour(&rects, i, Step::Down)` is where
an arrow key goes from item `i`, by where things are. Each card is
`style::card(painter, r, palette, hover, selected)`: rounded by
`metrics::CARD_RADIUS`, a hairline edge and a soft shadow, its content
`metrics::CARD_INSET` in; selected, it lies in the selection's colours
on a deeper shadow (picked up, not outlined). Its colours are
`style::card_ink(palette)` (`CardInk`: `gutter` between cards, `card`,
`edge`, `hover`, `text`, `dim`, and `well` for something set into a
card, a message's balloon; `dim` and `well` are the text colour,
see-through, so they read on a selected card too) and
`style::card_paper(palette)` the gutter and card as `Rgba` (a
`ScrollView`'s background). On dark the card is a lift above the paper,
not a darker hole in it. Don't head a card with a coloured band or a
little yellow tab: a card is not a window, and a tab on every one
repeats the window's own. Smaller pieces in
`style`: `pill(painter, x, cy, text, font, fill,
ink)` (a badge or count; `pill_width`, `pill_height`), `ink_on(fill)`
(black or white, whichever reads on an app's own colour) and
`monogram(painter, r, letter, fill)` (a lit, rounded letter tile: the
icon of something with a colour but no icon, a website or a contact).
`icons::paint_round(painter, r, tex, ring)` draws a person's picture in
a circle with a hairline ring; `icons::paint_cover(painter, r, tex,
radius)` a photo or artwork covering `r`, cropped about the middle,
never stretched. Notifications: see `olo_kit::notify` under
Notifications.

Text fields behave the same everywhere: click places the caret,
double-click selects a word, triple-click the line, and right-click opens
Cut / Copy / Paste / Select All, keeping the selection it landed on.
`TextControl` and `TextView` do this themselves; an app with its own
`egui::TextEdit` calls `widgets::edit_menu(ui, id, &response, &mut text,
editable, password)` after adding it (an editor that keeps more than text,
like StyledEdit's styles, opens a `context_menu` with its own Edit commands
instead). Nothing may sit on top of a `TextEdit` that senses clicks, or
double and triple clicks never reach it. `menu::request_paste(ctx)` pastes
the clipboard into the window as ⌘V does.

Colour emoji (`olo_kit::emoji`, on `crates/emoji`): egui's text is
monochrome, so code that places glyphs itself calls
`emoji::paint(painter, rect, cluster)`, which draws the cluster in colour
fitted and centred in `rect` (a cached texture at device resolution) and
returns false when there is no colour glyph (draw text instead).
`emoji::wants_color(cluster, wide)` says whether a cluster has emoji
presentation; `emoji::continues(prev, next)` joins sequences a cell grid
split (ZWJ, skin tones, flags). `olo-emoji` shapes with harfrust and
paints COLRv0/v1 with tiny-skia (Noto Color Emoji is COLRv1); the font is
read on a worker thread when the system fonts load. Render test images
with `EMOJI_OUT=/tmp/dir cargo test -p olo-emoji`.

Pictures (`olo_kit::images::open(path)`, re-exporting `crates/images`
so app_server can use it without the kit): everything the `image` crate
reads plus JPEG XL (jxl-oxide), AVIF (avif-parse and rav1d built without
asm, with our own YUV to RGB and alpha) and HEIC (heifer), EXIF-rotated,
with a 1 GiB allocation limit; returns the image and its format's name.
Keep its dependencies permissive: zenavif / rav1d-safe were dropped for
being AGPL. `OLO_SAMPLES=dir cargo test -p olo-images -- --ignored`
decodes example.avif and example.heic from `dir`. Thumbnails and Quick Look use
it. File panels (`olo_kit::file_panel`): `FilePanel::open(dir)` /
`FilePanel::save(dir, name)` give a BFilePanel (path popup, Home, sortable
Name/Size/Modified list that follows the folder as it changes, "Save
as:" field, Replace confirmation);
`FilePanel::folder(dir)` picks a folder (folders only; Select takes the
selected one, or the one shown). `.types(&["pdf"])` offers only those
extensions; `.filters(vec![Filter::new("Images").mime("image/*"),
Filter::new("PDF").glob("*.pdf")], current)` offers a Show pop-up of them
(by glob, ignoring case, or by MIME type, a kind of one counting:
`olo_xdg::mime::is_a`; which filters each file fits is worked out with
the listing, off the UI thread, so switching is instant; `panel.filter()`
says which is shown). `.multiple()` lets an Open panel choose several
files (⇧ and ⌘ clicks), answered as `Several(paths)`. `.choices(vec![Choice
{ id, label, options, value }])` adds an app's own choices above the
buttons (a checkbox when there are no options, else a pop-up, the pop-ups'
fields on one divider with Show's); `panel.choices_made()` reads them
back. Open it
in its own window of `PANEL_SIZE`, call `panel.ui(ui)` each frame and act
on the `PanelResult` (`Chosen(path)` / `Several(paths)` / `Cancelled`).
Open and folder panels give the list the keyboard when they open
(`ColumnListView::autofocus`, for any dialog that is mostly its list).
`olo_xdg::mime::from_name` finds a name's type from shared-mime-info's
globs, indexed by suffix (most are `*.ext`), so typing a big folder is a
few lookups a file. Toolbars are
`metrics::TOOLBAR_HEIGHT` tall (a row of controls, `TIGHT` above and
below), with their line under them (`style::hline` at the bottom); what
follows starts a line lower (`style::hairline`), so at 1.5× it is a pixel
down, not a point and a half, and the controls stay centred. `style::find_font(families, weight)`
loads an installed font as `(bytes, face index)` (call it off the UI
thread); `style::add_document_font(family, bytes, index)` makes it
`FontFamily::Name(family)` in every window of the app, including ones
opened later and after a reconnect, with the UI fonts as fallback. Check
`style::family_ready(ctx, family)` before naming it in a frame: egui
panics on unknown families, and new fonts arrive with the next frame. App settings live in
`olo_kit::config_dir()` (`$XDG_CONFIG_HOME/olo/NAME.json`); user
folders come from `olo_kit::user_dir("PICTURES")` (user-dirs.dirs,
None if unset or missing). Bytes too many to keep in memory (a download
being gathered) go in `olo_xdg::scratch_file()`: a nameless file under
`olo_xdg::cache_dir()`, on disk rather than /tmp, gone once closed.

Menus: `Menu::new("File").item(label, command, Some(Key::N))`,
`.item_shift(…)`, `.separator()`, `.submenu(Menu)`, `.checked(bool)`,
`.enabled(bool)`, `.icon(name)` (the last item's icon, any name
`icons::paint` takes; the generic application's if it can't be found),
`.all_enabled(bool)` (every item, submenus' too: a Font menu on a
read-only document). A submenu with nothing enabled in it is drawn
disabled and doesn't open, as a BMenu disabled whole
(`Menu::any_enabled`); its shortcuts and agents' `invoke` do nothing.
A popup with any icons gives them a 16 px column, after the check marks
when it has both. Popups are real `xdg_popup`s drawn by the kit. One
taller than the screen is shortened by the compositor (the positioner
allows `ResizeY`) and scrolls, as on BeOS: arrows at its ends scroll
while the pointer rests on them, and the wheel and arrow keys scroll
too; still, split very long lists into submenus (StyledEdit's font
families). ⌘H/⌘K/⌘U/⌘W never reach a focused text field as egui's emacs
deletions: the kit holds them back from `ui` and hands them to the menus
(windows with `raw_keys` get them as usual). While a menu is open the
window hasn't the keyboard and egui collapses a text field's selection;
keep your own copy if a menu command acts on it (StyledEdit's `blurred`).
app_server hands the keyboard back to the window as soon as its last
popup is destroyed (ending smithay's popup grab first, which ignores a
focus change until the popup's surface has gone too; and, the popup
gone, its window found from what had the keyboard when the grab began),
so Escape or an item chosen leaves the window typing again at once.

Alerts: `widgets::alert(ui, icon, message, buttons)` (or
`widgets::alert_frame(ui, icon)` for an alert with fields, which draws the
stripe and icon and returns the content column; `alert_text(ui, column,
top, text)` writes wrapped text in it and `alert_buttons(ui, column,
buttons, ready)` puts the buttons along its bottom, the default one
disabled until `ready`, with Enter and Escape) in a window opened
with `WindowOptions::alert(title, message)` (sized for the message);
`widgets::alert_choice(ui, icon, message, choices, &mut chosen,
buttons, cancel)` also asks which of `choices` (a radio button each
under the message when there are two or more, ↑ ↓ moving between
them), Escape choosing button `cancel` (so another button, such as
Passwords' "Use Another Device…", can come first at the left), in a
window opened with `WindowOptions::alert_choice(title, message,
choices.len())`;
an alert with fields is fixed at a height it works out before its first
frame, with `style::plain_text_height(text, width)` for its wrapped
text (the column is `ALERT_WIDTH`, or the window's own width, less
`ALERT_TEXT_X` and `metrics::WINDOW_INSET`): the UI face's own advances
and line height, wrapped as egui wraps, never a line short. A fixed
window (min size = max size) is always the size it asks for: app_server
doesn't give it a size remembered from before a restart. Icons are BeOS's `warning`, `stop`, `info` and `idea`. The kit makes the
alert a child (`xdg_toplevel.set_parent`) of the app's focused window, or
of `WindowOptions::parent(id)` (or `parent_opt(Option<id>)`) when given, and app_server opens any
window with a parent over it as BAlert does: centred across, a quarter
of the way down. An alert with no parent (a modal xdg-dialog naming
none: asked from the desktop, a menu or Deskbar, where no window of its
app had the focus, or a toolkit's parentless message box) opens in the
middle of the display the pointer is on, kept off Deskbar's column, in
front and focused (`place_alert`), and stays over all its app's windows
until it's answered (`keep_children_above`, BAlert's app-modal feel).

### Sidebars, dashboards and live graphs (`olo_kit::sidebar`, `dashboard`, `graph`)

`Sidebar::new(id, &rows, &mut chosen).show(ui, rect)` is a source list
down a window's left (Activity Monitor's sections): `Row::new(icon,
label)` with an optional quiet `.value("12%")` at the right, and
`Row::heading(label)`s over groups of rows; ↑ and ↓ step over the
headings while it has the keyboard; agents find each row as a selectable
`ListItem` with its value. Its rows are on a grid of their own (the
review doesn't hold them level with the content beside it).

`dashboard` is cards on the card gutter (`gutter(ui, rect)`):
`Card::new(title).figure("12%").caption("…").clickable(true).show(ui,
rect)` (`.figure_ink(colour)` for a figure that needs attention, in the
palette's `warning` or `failure`) gives the content rect under its head (`Card::height(content,
caption)` the height it needs; a clickable card is a button named by its
title, its figure the value); `facts(ui, rect, &[(label, value)])` (and
`facts_two`, `facts_in` for a panel's colours, `facts_label_width` to
line a control up with the values) are label: value rows; `legend(ui,
rect, &items)` swatches, names and values that wrap (`legend_height`;
a transparent `LegendItem` colour is a figure with no swatch); `meter(ui,
rect, &[(color, part)], total)` a rounded bar of parts of a whole;
`flow(area, n, min_width, max_columns, height)` lays cards out in as many
columns as fit. Each card is a grid of its own for the review.

`Graph::new(id, &series, now).span(s).max(m).unit(&f).show(ui, rect)` is
a live graph of `Series::new(name, color, &samples)` (time, value pairs,
oldest first; NaN a gap; `.fill(true)` a faint fill under the line; a
series of another unit `.top(t)` and `.format(&f)` of its own): time
runs right to left, `now` at the right edge, so it scrolls smoothly
between samples, asking for the next frame only when they have moved a
device pixel (a minute over 600 pixels is 10 frames a second, not 120
drawing the same pixels); a point a device column at most (peaks kept);
hovering shows a time line and each value then, and `GraphResponse`
says the time hovered or clicked (to look back: Activity Monitor's "At
2:04 PM" card). `.plain()` drops the sunken well, for a card.
`nice_ceiling(v)` is a scale's top.

### Searching and filtering (`olo_kit::search`)

Every app's search is the same language, field and filter bar. The
language: words (each must match, any order, ignoring case),
`"phrases"`, `-word` to leave out, and the app's filters (`Facet`s) as
`key:value`, `key:a,b` for either, `-key:value`, `key:"two words"`; a
`key:` the app doesn't know is a word (`https://…` finds itself), a part
still being typed (`key:`, `-`) asks nothing. `Query::parse(text,
&search::keys(&facets))`, then `query.matches(|key, needle| …)`: the app
says whether an item has the needle under the key (`None`: anywhere in
its text); `Needle` matches without case, memchr-fast for ASCII
(`in_text`, `is`, `starts`, `find`, `find_all`). A value being typed is
best matched by its start (`starts`), so the list narrows as it's typed.
`query.score(&[(field, weight)])` ranks (whole field 8, its start 4, a
word's start 2, anywhere 1, times the weight) to put the best first;
`search::highlight(text, &query, font, color, search::mark_color(&p))`
marks what was found (not on a selected row's colours).

`SearchField::new(id, &mut text).facets(&f).hint("Search")`: a
magnifier, the text with filters' keys in blue and left-out parts in
red, a clear button, and while typing a list under it (an `Area` over
the window, so the field keeps the keyboard) of the filters and values
that fit the word at the caret (`search::suggest`): ↑↓ choose, Tab takes
the chosen or first, Return the chosen, Escape closes the list, then
clears, then lets go; a press takes one (on the press: the editor lets go
of the keyboard when pressed outside it). `SearchField::focus(ctx, id)`
for Find (⌘F), its text selected. `FilterBar::new(id, &mut text,
&facets)`: a pill per facet in the bar (`Facet::typed_only` keeps one out),
"Label: Any ▾" or what's chosen (bold, tinted), whose menu has Any and
each value with its count where a shortcut would be; choosing one rewrites
the query's text (`search::set_filter`), so the field, the bar and
agents typing a query always agree. `.no_clear()` drops the bar's Clear.
Facets are built from the data (values and counts) and kept until it
changes, not made every frame.

### The text editor (`olo_kit::editor`)

For documents (StyledEdit's window; anything a person writes at length)
use `editor::Editor`, BTextView, not egui's `TextEdit`: that lays out,
copies and hashes the whole text every frame, which a long document
can't do at 120 frames a second. The editor keeps a galley per
paragraph, laid out only while it shows; the rest have estimated
heights, those within a screen of the view are measured before they come
into it (a millisecond's worth a frame), and the view holds on the
paragraph at its top while heights above it settle, so nothing jumps.
`TextControl` and `TextView` stay for fields and short text.

The app's text and styles are an `editor::Document`: `text()`,
`sections(range, job)` (one paragraph's styles as `LayoutJob` sections)
and `replace(range, text, selection)`, through which every edit goes
(typing, pasting, a drop), so undo and styles follow; `copied` and
`move_text` are optional. Each frame, `editor.show(ui, rect, &mut doc,
&Options { editable, wrap, align, justify, margin, background })` gives
`Output { response, changed }`. What the app changes itself it reports:
`reset(text)` for new text (loaded), `changed(old, new)` or
`replaced(text, removed, inserted)` for an edit (undo, Replace All),
`restyled(range)` for new styles, `relayout()` for new fonts or
alignment. `selection()`, `caret()`, `select(text, range)` (and show
it), `line_col(text, at)` for a status line. `view()` is where the view
is (a paragraph's first byte and how far into it), and `set_view(text,
view)` puts it back, for `save_state` through a reload. Positions are
bytes.

Keys are a Mac's (⌥ or ⌘ with arrows by word or to either end; Home/End
along the row; Page Up/Down; Shift extends). A double click selects a
word, a triple click the paragraph. A press on the selection drags its
text (`text_drag`); text dropped on it goes in where it lands, and moves
within the editor. Dragging a selection past an edge scrolls
(`ScrollView::edge_scroll`). Agents read it as a `MultilineTextInput`
with the whole text and where it shows (read-only when it isn't editable). Justified and
centred rows that end in spaces use a patch to epaint (vendor/epaint,
see vendor/README.md).

### Look and feel rules

1. **Colours only from the palette** (`style::palette(ctx)` or
   `cx.palette()`): `panel_background`, `document_background`,
   `control_*`, `menu_*`, `list_*`, `shine`, `keyboard_navigation`, … Use
   `style::shade(&p, rgba, tint)` for bevel shades (it compensates in dark
   mode). Never hard-code a grey: it will be wrong in the other theme.
   Semantic accent colours (graph lines, file-type tints) must be checked in
   both themes.
2. **Spacing only from `olo_kit::metrics`** (`SPACING` 8, `TIGHT` 4,
   `WINDOW_INSET` 12, `BOX_INSET` 10, `LABEL_GAP` 8, `CONTROL_HEIGHT` 24,
   `LIST_ROW` 20, …). If you need a new kind of gap, add a named constant
   there rather than a literal in your app.
3. **Text**: draw single lines with `style::label(painter, x, center_y,
   align, text, font, color)`; it centres capitals optically and snaps to
   device pixels. Any line that can be longer than its room (a title, a
   path, a name the person typed) goes through `style::label_fit(…,
   max_width)` instead, which ends it with "…" rather than clipping a
   letter in half or running into its neighbour (agents still read the
   whole text). Counts people read have their thousands separated,
   `style::thousands(n)` ("30,001 actions"). Fonts: `style::plain_font()`, `style::bold_font()`,
   `style::italic_font()`, `style::mono_font(size)`. The faces are Noto,
   shipped with Olo in `data/fonts` (see its README: Noto Sans and Noto
   Sans Mono, variable, with Symbols, Symbols 2 and Math for what they
   lack; OFL) and installed to `share/olo/fonts` (`install-session.sh`,
   and an upgrade's `sync_fonts`); `olo_xdg::fonts_dir()` finds them,
   and the kit (`style::font_definitions`) and app_server (`text.rs`) both
   use them ahead of the system's, so Olo looks the same on any system.
   `olo_theme::fonts` lists the faces in the order they are tried (`UI`,
   `MONO`, `SYMBOLS`; the system's DejaVu Sans, CJK and emoji after).
   `style::CAP_HEIGHT` is Noto Sans's; app_server centres its own text
   (tab titles, Twitcher, hint badges) with `text::top_for_centre`, from
   the face's own metrics. Glyphs the primary
   face lacks (▸, →, ⌘) come from fallback faces the kit shifts onto the
   primary's baseline, so they sit on the line with the letters;
   `style::baseline_offset(galley)` is that baseline even when a line
   starts with one. Highlight selected
   text with `style::text_highlight(galley, origin, size, pad)` (cap height
   to baseline, so every label's highlight is the same height); drag-select
   bands with `style::selection_band`. Keys as on a Mac menu ("⇧⌘←", each
   symbol on the text's baseline, though they come from a fallback font;
   one about as tall as the capitals, ⇧ ⌘ ↵, stands by its ink where they
   do, whatever its face's metrics say):
   `style::key_combo(painter, right_center, keys, color)`. A failure
   beside text (an error's title, a failed row) leads with
   `style::stop_mark(painter, rect, palette)`, a crisp vector stop sign
   (12 px reads where an alert's icon would be mud); set it level with
   the text's capitals with `style::cap_middle(galley, size)`. A pull
   request is GitHub's mark, `style::pull_request_mark(painter, rect,
   PullRequestShape::{Open, Draft, Merged, Closed}, color, node)`, 16×16,
   its lines whole device pixels at every scale (its commits a whole
   pixel's ring at 1×), `node` filling the branch's commit (its checks'
   colour). Say what
   a thing is with its icon, a chip or a mark, not a coloured band or
   edge along it: those clash with icons and look unfinished.
4. **Selection looks the same everywhere**: the palette's list selection
   (`list_selected_background`, `list_selected_item_text`, BeOS's
   `B_LIST_SELECTED_*`), under or over the thing, never a thick coloured
   outline. `style::selection_ink(palette)` gives its `SelectionInk`:
   `fill`, `edge` (a hairline for a selected thing that stands on its
   own, so its shape stays crisp), `text` and `dim` (`text`,
   see-through, for captions on it). Pick by what is selected: a list
   row or an icon's name sits on `fill`; a card is `style::card(…,
   selected)`; a picture, swatch or preview stands on
   `style::selection_plate(painter, r, palette)` (a rounded ground
   `metrics::SELECTION_PAD` beyond it, painted first, as Tracker sets a
   selected icon on its highlight); a part of something flush with its
   neighbours (a bar's segment, a map's partition) takes
   `style::selection_wash(painter, r, palette)` (the fill, see-through,
   with its hairline inside); something round (a colour swatch) a ring
   of `fill` with an `edge` stroke. A group that is current as a whole
   (Deskbar's workspace showing) is lit: `style::lit_ground(palette,
   under)` behind all of it, `style::lit_edges` above and below, its
   rows shading that ground as they would `under`; a row chosen inside
   it takes the menu's selection (`menu_selected_background`), which
   stands out on it where the list's would not. `keyboard_navigation`
   is only where the keys are (focus), never what is selected. The edge
   colour is
   `Palette::selection_edge()`, so app_server's own drawing (Twitcher's
   chosen app, on the same plate) matches the kit's.
5. **Lines are one logical pixel** (`style::hline`, `vline`, `frame_rect`,
   `bevel`, `sunken`, `raised`), made a whole number of device pixels:
   `style::line_pixels(ppp)` (1 at 1× and 1.5×, 2 at 2×), so at every
   scale all lines are one weight and crisp. A line ending at an edge
   is `hline_above(…, bottom, …)` / `vline_left_of(…, right, …)`, not
   `hline(…, bottom - 1.0, …)` (a pixel off at 1.5×); a stroke is
   `Stroke::new(style::hairline(ctx), c)`, not 1.0 (it blurs at 1.5×);
   what fills inside a frame of n lines is `style::inset(ctx, r, n)`,
   not `r.shrink(n)` (a crack at 1.5×). Snap rects to the pixel grid
   (`style::snap`; something of a set size centred on a point,
   `style::snap_centred`, which keeps its size; a line that icons and
   text are centred on, `style::snap_coord`, else at 1.25× an icon
   snapped from it and text centred on it round a pixel apart), and lay
   out things that
   tile (panes, a bar's segments) in device pixels, so no seam opens
   between them. No blurry half-pixel lines. Heights and widths are on
   the 2-point grid (even numbers of points: `metrics`, `menu`, a
   button's width), so they are whole device pixels at 1×, 1.5× and 2×
   and nothing is left a pixel over (`kit_sizes_on_the_two_point_grid`
   checks the kit's). Two fonts side by side on one line share a
   baseline: `style::label_with_baseline` (or `label_fit_with_baseline`)
   for the first gives it, `style::label_on_baseline` puts the others on
   it (each centred on its own lands a pixel apart at 1.5×); a mark
   beside a galley you placed centres on `style::cap_middle`. A row whose
   height is worked out (from its text) is rounded up to the 2-point
   grid, not to device pixels: egui keeps sizes in 32nds of a point, so a
   row of whole device pixels at 1.5× is a hair off one, and a list of
   them drifts a pixel. See "Fractional scaling". An outline's fold mark
   is `style::latch(painter, rect, open, color)` (BOutlineListView's
   triangle: right when folded, down when open).
6. **Icons**: `olo_kit::icons::paint(painter, rect, "name")` with names
   from `data/icons/manifest.txt` (or freedesktop names / absolute paths).
   Sizes 16 (lists, Deskbar), 32 (Tracker list), 48–64 (Tracker icon view),
   128 (About boxes). An icon is drawn at its texture's own size, a
   texture pixel to a device pixel, centred in the rect; a size Olo has no
   bitmap for (20 at 1.25×) is averaged down from the next one up when it
   loads. Icons and the wordmark are drawn in the theme chosen
   in Preferences ▸ Themes, and redrawn when it changes; never keep an
   icon's texture yourself. Olo's wordmark is `olo_kit::brand::wordmark(painter,
   rect, dark)` (from `data/brand` or the theme's `brand/`, shrunk crisply to the height asked
   for; `wordmark_width(h, dark)` to lay it out, as the light and dark
   versions can differ in shape; `wordmark_centred_in` centres it across a
   rect on device pixels, for a column of them); Deskbar's tile shows it alone,
centred (beside the `olo` mark it read as "o olo"). Image previews: `icons::thumbnail(ctx, path, size)`
   decodes off-thread and returns `None` until ready (it repaints when
   done); place it with `icons::fit(ctx, &tex, bounds)` so it is never
   upscaled or blurred. Need a new one? Follow **[ICONS.md](ICONS.md)**: the
   style guide, subject template and required generation workflow
   (sunburst at max quality, against the reference set, candidates
   reviewed on a contact sheet).
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
   it was given or over a neighbour, and no fixed text is cut short. A
   fixed-size window (a preference panel, a dialog) proves it with a
   layout lint test (see "Layout lint" under Testing). Kit containers and
   controls take part by themselves; a view that marks out an area or
   draws a control by hand notes it with `layout_lint::area` /
   `layout_lint::control`, and a new kit widget calls them too.

## The session and in-place reload

Olo upgrades without logging out. `olo-session` (what the login
screen runs) owns the Wayland socket (`wayland-N`) and the IPC socket and
hands both to app_server, so clients never see the sockets disappear.

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
  reconnect in: `layout.rs`).
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
  leases finish, tablets are removed). Set `OLO_WLPROXY_DEBUG=1` for
  the session (`tsession` passes it on) to log every replayed request. Set
  `OLO_NO_WAYLAND_PROXY=1` to have app_server take `wayland-N` itself.
  If the proxy keeps crashing, the session does that on its own.
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
  `$XDG_RUNTIME_DIR/olo-wayland-N-direct` (the kit's `connect_once`).
  They reconnect and re-execute by themselves, as below.
* Test with `tsession start NAME --session`. Run an outside program in
  it with a fixture `HOME` and a private bus (`tools/dev/busd-lenient`),
  then run `tsession reload NAME` a few times and check it still draws,
  takes input and opens menus.

* `olo-session reload` (SIGHUP): if `olo-session` itself was
  upgraded it re-executes the new binary first, keeping its children and
  sockets. Then it stops app_server (SIGTERM; it saves every window's
  place to `$XDG_RUNTIME_DIR/olo-layout-<display>.json`) and starts the
  one now on disk, which puts returning windows back (same app id and
  title within 60 s; position, size, workspace, minimized, focus).
  `layout.rs` decides what is returning: the same process on a new
  connection (title, else app id, within a minute), or an app started
  again after its old process ended, with the very same app id and title
  within 15 minutes (Chrome restarting restores its windows; closed
  windows are remembered in memory, up to 200). An app started again
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
  focused window, else under the pointer), including a running app
  opening a window again.
* Kit apps notice the lost connection and reconnect (for up to 20 s). An
  app whose binary didn't change keeps everything and recreates its
  windows exactly (at their current sizes: a panel's last `cx.resize`
  too, since apps like Deskbar only resize when their height changes).
  One whose binary changed calls `App::save_state`,
  re-executes itself (same pid and arguments), and reads the result with
  `cx.restored_state()` at start-up, unless `App::busy` says it is doing
  something re-executing would cut short (olo-portal sharing a screen):
  then it reconnects as the old version and becomes the new one once
  `busy` turns false. `App::upgrade_due` says a new version is waiting
  (once, before `busy` is asked), for an app to stop taking new work
  and finish what it has (olo-pool's drain). In an upgrade olo-session
  signals every program it replaced (SIGURG) and restarts app_server at
  once, so an app re-executes while the old app_server goes: one may
  start against the old one and lose it in its handshake (it tries again
  inside a session, as reconnecting does); a surface may be made, or fail
  to configure, as it goes (`Surface::gpu_configured`, checked with an
  error scope: wgpu panics, release builds too, at drawing on an
  unconfigured surface; configuring is tried again at the next frame); a
  frame not drawn keeps its texture changes for the next one
  (`Surface::pending`). A GPU surface lost while app_server
  is away waits for the reconnect (`configure_gpu_surface` checks
  `is_surface_supported`; `present` skips a surface with nothing to draw
  with), and wgpu's errors are logged, not fatal (`on_uncaptured_error`):
  no app may die of an upgrade.
* Nor of losing the GPU (a driver reset: `VK_ERROR_DEVICE_LOST`, as
  Asahi's Mesa gives at a failed syncobj transfer). Everything the device
  made is gone, and wgpu and egui_wgpu panic at using it. The kit
  notices from wgpu's device-lost callback, or a panic in GPU work on
  any thread (`on_gpu` wraps presenting, setting up surfaces, menus made
  ahead and texture uploads), stops drawing and starts the app anew as
  an upgrade does (`recover_gpu`: `App::save_state`, re-execute, same
  pid): the textures the app holds (pictures, pages) went with the
  device, and only the app makes them again. Lost again within 10 s of
  that, the GPU is broken, not reset, and the app ends rather than loop.
  app_server puts a window back on its own as it was in every way: a
  window remembered by this app_server whose own process came back
  (`Placement::id`) is stacked among the others by the windows that
  were under and over it (taken once as its client's windows begin to
  go, `closing_stack`; found again by `WindowMeta::was`), and the
  keyboard and the active window it took as it opened go back to what
  had them (`WindowMeta::focus_before`). Deskbar keeps a gone app's
  place in its list for 20 s, and saves its order to re-execute with.
  Test with the kit socket's debug-only `{"lose_gpu":true}`
  (`olo-menus-PID.sock`), which destroys the device.
* **Every app with windows the user
  arranged must implement both**: save which windows are open (with
  `cx.window_size`), their documents/folders, view settings and
  selection, and reopen them in the same order. Tracker's `Saved` is the
  example.
* app_server or a shell component that crashes is restarted (up to 5
  times a minute). `olo-session quit` ends the session.
* olo-session's watchdog (`crates/session/src/watchdog.rs`) pings
  app_server over IPC every 2 s. After 10 s without an answer it sends
  SIGABRT (so `coredumpctl debug` has the stack that hung), then SIGKILL
  10 s later, and restarts it like a crash: apps reconnect and a locked
  screen comes back locked.
* Never cache anything about the connection you can't rebuild; never
  replace an installed binary by writing over it (install to a temporary
  name and rename, as `tools/install-session.sh` does).

On a fresh Fedora machine (x86_64 or aarch64, Workstation or atomic),
`tools/setup.sh` installs what's missing, builds and installs; `--check`
only reports. `tools/dev/x86-check` type-checks the workspace for x86_64
from an aarch64 machine (x86 headers unpacked under ~/.cache; nothing
installed).

Install for the login screen with `tools/install-session.sh` (as
yourself; it asks for sudo once). It builds release binaries into
`~/.local/bin` (so upgrades need no root), icons into
`~/.local/share/olo`, and the session entry into
`/usr/local/share/wayland-sessions/olo.desktop` (Exec is the installed
path; no TryExec, as the greeter can't look inside a private home).

**Upgrades from the build tree** (`crates/session/src/upgrade.rs`): the
installer records the tree's `target/release` in
`~/.local/share/olo/build-dir` (`OLO_UPGRADE_FROM` overrides it).
Every 3 s the session compares the `olo-*` programs there with its
own. It skips this while cargo holds one of its locks in the tree
(`.cargo-lock`, or `.cargo-build-lock`/`.cargo-artifact-lock` in newer
cargo) or a program is under 3 s old. It lists newer ones in
`$XDG_RUNTIME_DIR/olo-upgrade-<display>.json`; Deskbar follows that file
(`olo_kit::watch`) and shows a green arrow in its tray. Clicking it asks first. Accepting,
or `olo-session upgrade` (SIGUSR2), installs them beside the session:
a btrfs/XFS clone (`FICLONE`) or a copy, under a temporary name and then
renamed. Only a new `olo-app-server` needs a reload. Other programs
keep their windows through it (above) unless they came without the proxy
(`WindowInfo::kept` is false), and Deskbar's alert names those first. Otherwise app_server keeps running: the session
sends SIGURG to each of the user's processes whose program file in the
install directory was just replaced (`/proc/PID/exe` ends in
" (deleted)"). The kit catches it (a self-pipe in `platform.rs`), saves
state and re-executes, as after a reload, and app_server puts its windows
back where they were. A new olo-session re-executes itself without a
reload. When app_server is new too, the nudge comes just before the
reload, so apps re-execute while app_server restarts: that race is why
a window not shown yet keeps the place it claimed, and why the kit
survives a lost surface (above). Test it with
`TSESSION_SESSION_EXE=<install>/bin/olo-session
OLO_UPGRADE_FROM=<tree> tsession start NAME --session --shell` and
`touch` a program in the tree. Make the install and the tree under
`/var/tmp` (`cp --reflink=always` the release programs into
`<install>/bin`, then again into `<tree>`), not `/tmp`, which is RAM.
For the race: touch an app's program, send the app SIGURG, then send
the session SIGHUP 30–120 ms later, and check the window kept its
place.

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
from then carry on. `olo_xdg::legacy` (re-exported as `olo_kit::legacy`)
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

## Testing: headless sessions

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
```

Each headless screen draws at its mode's rate (60 or 120 Hz), on time:
check anything that moves at 120 Hz too. Every kit app's `{"frames":true}`
(on its `olo-menus-PID.sock`) answers with each frame it drew since the
last ask, the last 1200 (`FrameMark`): when it began (CLOCK_MONOTONIC
µs, app_server's clock), how long it took, its parts (`setup_us`, a
surface made or a swapchain configured; `ui_us`, egui and the app's
code; `tessellate_us`; `acquire_us`, waiting for a buffer to draw in;
`submit_us`, uploads, recording, submitting and presenting), and where
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

Every session gets its own D-Bus session bus (busd, at
`/tmp/olo-dev/NAME/bus`, in `DBUS_SESSION_BUS_ADDRESS` for `run`): the
runtime directory is the real one, so without it programs fall back to
`$XDG_RUNTIME_DIR/bus`, the user's real session bus, and a test's
notifications pop up on their screen. tsession won't start without busd
(`TSESSION_BUSD`, or the build under `~/.cache/olo-dev`). To see a
test's notifications, run its own centre:
`$T run NAME olo-notifications --bus unix:path=/tmp/olo-dev/NAME/bus`.

sudo resets HOME, so without `--home` a session and its apps use the real
home folder: pass `--home` whenever a test saves files (Screenshot,
Tracker, StyledEdit…). Sessions are always silent (`PULSE_SERVER` points at
a dead socket), so tests never reach the real speakers or volume. To
test sound, run `olo-fakesound SOCKET` (olo-preview): it speaks the
PulseAudio protocol, plays nothing, and logs each stream once a second
(bytes consumed in real time, corked or not, loudness); start the app with
`$T run NAME PULSE_SERVER=unix:SOCKET …`.

The headless server renders on the GPU's render node and announces it
in dmabuf feedback, so GL clients (Chrome) get hardware GL
as on the desktop rather than Mesa's software fallback.

`tools/dev/wm-fuzz NAME [--seed N] [--steps N] [--only OPS] [--keep-going] [--tiles] [--trace-tiles]` is an
adversarial window-management fuzzer: on a session with two screens and
the shell (`$T start fz 2560x1600,1920x1080 --session --shell`) it opens
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
every window's frame as it changed during the step, with when. A run is
reproducible from its seed; `--only app_resize,tab_in` runs just those
steps (the `op_` methods). Run it after changing anything in shell/.

`tools/dev/auto-fuzz NAME [--seed N] [--steps N] [--only OPS] [--keep-going] [--reloads]`
is its auto-tiling counterpart (built on it, on the same kind of session):
it turns Auto-Tiling on for the first four workspaces and, mostly, drags
the seams between tiles the way people do (fast and slow, a few steps or
many, past where it stops and back, held still before letting go), with
windows opening, closing, grouping, ungrouping and changing tabs and
workspace trips between; `--reloads` restarts app_server now and then.
After each step, once the windows settle, every tile (one per group) must
be inside the tiling area with no two overlapping, and stay put for a
moment after (nothing still on its way to land somewhere else). Each
step's line has the time, to find it in the session's log. Run it after
changing anything in `autotile.rs`, `tiling.rs` or the resize grabs.

`tools/dev/upgrade-check NAME [--times N] [--reload-only] [--no-touch]`
checks that windows come back from an upgrade exactly (display,
workspace, frame, minimized). Arrange a `--session --shell` tsession
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
anything that runs per frame or per event.

`tools/dev/bench` is the speed benchmark, every app against the 120 Hz
budget (rule 9). On release builds (`cargo build --release --workspace
--bins --examples`: `--examples` alone builds only the kit's demo;
`TSESSION_PROFILE=release` makes tsession run them) it
starts a session of its own ("bench": a 3456×2234 screen, a 16" MacBook
Pro's, at 120 Hz, with the shell) and a sandbox home under
/var/tmp/olo-bench filled with what makes apps work (a 20,000-line
document, a folder of 3,000 files, photos, 200 notifications, a
terminal's worth of output), and at 1×, 1.5× and 2× runs each app
(the kit's demo, Tracker twice, StyledEdit, Terminal, Viewer,
Notifications, Activity Monitor, DeskCalc, DiskUsage, Logs,
Preferences, the palette, Passwords, Screenshot) through what people
do: launched and zoomed to fill the screen, left to load, idle for a
second (it should draw nothing), the pointer swept over it, each scroll
view scrolled (fingers, a flick and its glide, wheel notches, judged as
scroll-check does), its tabs clicked, its menus opened and walked, a
field typed in, the window resized live, the theme flipped; then
Deskbar and every screen saver module. Every frame each draws comes
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

### Layout lint (`olo_kit::layout_lint`)

Whether a window's content fits depends on fonts, wrapping, the scale and
what it is showing, so the lint runs the real UI code rather than reading
it: `layout_lint::check(size, |ui| view(ui))` (or `assert_fits(name,
size, view)`) runs a view headlessly in plain egui (no GPU, no Wayland,
the kit's own fonts and style, as `platform.rs` makes a window's context)
at the size its window gets, in light and dark at 1×, 1.5× and 2×, three passes
each, and returns one line per problem with the looks it shows in:

- an area's content running past it (more than half a point): a tab
  view's panel, a `BBox`, `window_content`, or one a view notes itself
  (Preferences notes each panel's body above Defaults / Revert);
- a control outside the window, or over another control on the same
  layer (`Touch` running under Defaults);
- text that doesn't fit: a label closer than `TIGHT` to its field
  (`MenuField`, `TextControl` dividers), a `Slider`'s label and value or
  limits running together, tabs wider than their view, a `StringView`
  cut short with "…".

Kit containers and controls note themselves while a check runs (one
atomic load each otherwise). Inside something that clips (a
`ScrollView`) content may run on, so only what shows counts. Lint tests
are `#[test]`s named `ui_lint_*`, so `cargo ui-lint` (an alias in
`.cargo/config.toml`, which lists the crates that have them) runs them
all, and `tools/install-session.sh` runs it before it builds: a window
that doesn't fit stops the install. They're ordinary tests too, so
`cargo test` runs them.

Give the view the longest things it may show, as real machines give them:
Preferences' `ui_lint.rs` runs every panel and every tab with made-up
devices (a MacBook pad with no pressure and the driver's click
thresholds, long Bluetooth and sound device names, a long processor
name, displays, networks, fingers); the panels' workers start nothing
under `cfg(test)` and their shared state starts filled
(`touchpad::State::showing(tab, devices)`, `keyboard::State::showing(tab)`,
`about::State::sample()`, `Server::detached()`, …). A new fixed-size
window or dialog gets a lint test the same way.

### Layout review (`olo_kit::layout_review`)

Where the lint proves things fit, the review reads one frame's painted
shapes (text with its baseline, filled bands, lines, icons) at the scale
it was drawn at and flags what tends to look off: what you should look
at closely before calling a view done.

- **Errors** (tests fail on them):
  - `blurry-icon`: an icon off device pixels, not a whole number of them,
    or a pixel or two off its texture's own size (`Marks::textures`), so
    it is resampled.
  - `line-weights`: one-point lines of different weights.
  - `blurry-outline`: a stroke not a whole number of device pixels.
  - `seam`: a line and what it borders a device pixel apart.
  - `off-pixel`: a fill with an edge between device pixels. egui rounds
    each edge, so the fill comes out a pixel off what it lines up with:
    rows of two heights, or a bar a pixel off the band beside it. A line
    is judged only across its thickness, and edges a clip cuts or on the
    window's own edges are skipped. The check runs only at scales where
    the 2-point grid lands on pixels (1×, 1.5×, 2× and up), not at 1.25×
    or 1.75×, where edges are still crisp but spacing may differ by a
    pixel.
- **Review** (most are mistakes, a few are meant): `near-height` (bands
  stacked or side by side almost one height: a 21 bar over a 22 head),
  `near-level` (text or icons in neighbouring columns a pixel or two off
  level), `near-edge` (left edges almost lined up down a column),
  `off-centre` (text or an icon off-centre in the band it sits in; only
  bands no clip cuts, since a card at the foot of a scroll view is
  centred in the whole of it, not in what shows), `uneven-rows` (rows
  down a column spaced unevenly). Right edges count as lined up a
  device pixel apart: text set from its right starts on a pixel.

A view tells it two things it can't see. `layout_review::own_grid(ui,
rect)`: text laid out on a grid of its own (Terminal's screen, a cell
apart; a card in Notifications' history masonry, staggered in its
column), not held level with what is beside it, nor in a row with
what is above it. A scrolled `ScrollView` marks its content so itself:
a list's heading is a row apart from its first row only at whole-row
offsets. `layout_review::band(ui, rect)`: what is in `rect` is centred
across it though nothing fills it (a `TabStrip` tab, its top below the
strip's), so centring is measured against it. Call either each frame.

Run it:

- in a test: `layout_lint::review(size, view)` gives every finding, in
  light and dark at 1×, 1.5× and 2×, one line each with the looks it
  shows in; `layout_lint::check` (and so every `ui_lint_*` test) fails
  on its errors. `cargo test -p olo-preferences ui_review -- --nocapture`
  prints every panel's findings; `kit_widgets_pass_the_review` fails
  on any finding at all in the kit's own widgets, so they stay clean.
  Those include popup menus with shortcuts, separators, checks,
  submenus and icons. `kit_sizes_on_the_two_point_grid` fails on a kit
  size that isn't an even number of points;
- on a running app: every kit app answers `{"review":true}` on its
  socket with a `commands::WindowReview` per window (`commands::review(
  pid)`): `$T review NAME [--all]` on a headless session, `olo-screen
  review NAME [--window ID] [--json]` on an agent screen. Switch the
  scale (`$T scale NAME 1`, `1.5`, `2`) and theme and review each.

Go through each finding: fix it in the kit or the view, or, when it is
meant, tell the review why (`own_grid`, `band`) so the next run is
quiet.

Memory: measure release builds (`cargo build --release -p APP`, then
`grep -E 'VmRSS|RssAnon|RssShmem' /proc/PID/status`; RssShmem is GPU
buffers on Asahi). Baselines (October 2026, 2× headless): DeskCalc 54 MB
RSS (15 MB heap, 13 MB GPU); Terminal 79 MB idle, +29 MB for a full
10,000-line scrollback at 120 columns (alacritty's cells), +28 MB per
extra window (swapchain). Two kit rules keep it there: UI fonts are
mapped, not copied (`style::system_face`; the CJK face is 31 MB), and
the wgpu device asks for `MemoryHints::MemoryUsage` (the default pools
cost ~60 MB of GPU memory per app).

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

* Settings that live in app_server come over IPC (`server.rs`): a watcher
  thread follows `SettingsChanged` / `OutputsChanged`, a writer thread sends
  `SetSettings` (collapsed to the newest) and `ConfigureOutput`. The UI edits
  a local copy and diffs it each frame; nothing blocks.
* Each panel module has `ui(ui, &mut Settings, ..)`; per-window state goes
  in `panel::State`. Panels that change settings get Defaults and Revert
  (to the values when the window opened) along the bottom, each enabled
  only when it would change something: implement `Panel::reset`.
* New panel: add it to `Panel` (arg, title, icon, size, reset), write the
  module, add it to `PREFERENCE_PANELS`, journal it light and dark.
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
  off; Terminal's **Coding agents' tab titles**).
  Terminal follows them live (`settings::watch`).
* Windows (`windows.rs`) sets the space Olo leaves around the windows it
  arranges (`Settings::tiling`: margin at the display's edges, gap
  between windows), with a miniature of the main display tiled three
  ways that follows the sliders. app_server refits arranged windows as
  they change, so the sliders move them live. Deskbar's Window
  Arrangement menu ends with Windows Preferences….
* Network (`network.rs`) reads NetworkManager through
  `olo_system::network` on a worker thread (devices, addresses,
  networks in range merged by SSID, saved connections; a scan every 30 s)
  and joins (saved connection, or a new WPA-PSK one), disconnects and
  forgets. A new enterprise (802.1X) network's Join… opens a sheet
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
  state (`config_dir()` and `state_dir()`, as `config/…` and `state/…`)
  into one `.olo-backup` file: a zstd tar with `manifest.json` first
  (format, version, when, each file and its size). Import reads the
  manifest and shows it by app (`Manifest::groups`) with Import / Cancel;
  importing first saves what is there to `state_dir()/backups` (the last
  five; left out of backups), then replaces each file atomically, keeping
  its mode; names outside the two roots are refused, and files the backup
  lacks are left alone. app_server keeps its settings in memory, so the
  backup's `app_server.json` is sent to it as `SetSettings`. Both run on
  a worker thread with a StatusBar and Cancel (import can only be
  cancelled while it saves what is there). `olo-preferences --export
  FILE`, `--inspect FILE` and `--import FILE` do the same without a
  window; opening a `.olo-backup` (`mime::SETTINGS_BACKUP`, kit
  `Kind::SettingsBackup`) shows it in the panel, ready to import.
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
  its text field follows the setting when it changes elsewhere. app_server draws the wallpaper (`wallpaper.rs`):
  each output's picture is decoded (JPEG XL too, via jxl-oxide), scaled
  to cover and cropped on a worker thread (one picture at a time, only
  the latest asked for each output), kept in the output's user
  data and drawn under every surface; the workspace colour shows until
  it is ready or if the file can't be read. Scaling is in 8 bits with
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
  the main display changes or their output is unplugged. Those called up
  from the keyboard to work where the person is
  (`shell::OPEN_WHERE_IN_USE`: the palette) open on the display in use
  instead, `active_output()`: the focused window's, else the display a
  workspace was last switched on or the pointer last pressed on
  (`AnvilState::last_display`), else the pointer's. Every display has
  its own Deskbar (one layer surface per output, `Bar` in apps/deskbar,
  each naming its output: one that named none would follow the main
  display about), each listing every display's workspaces and their
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
  number, left to right from D0 (`olo_protocol::display_number`; the
  map's and the settings box's names, "D1 Studio Display"), which names
  its workspaces (`workspace_name`: "D1 Workspace 2").
  render.rs draws the layers itself, front to back: Overlay, the menus
  of the Background and Bottom layers (the desktop's right-click menu,
  `shell::lower_layer_menus`), Top (Deskbar), the windows, then the
  Background and Bottom surfaces; `surface_under` and keyboard focus
  find them in the same order, so a desktop menu opened over a window
  is in front of it and takes its clicks.
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
  (`choose`). The choice is
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
settings change) before measuring and drawing the tab.

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
The tray's status icons (upgrade, agent screens, battery, Wi-Fi,
volume, notifications, the Caffeinated cup and the approval key,
`status::draw_key`, `approve.rs`: see "Approving without asking") sit on
a 22 px grid in a recessed well, in as many rows as they need, shared
out evenly among them and each row centred (eight are two rows of
four).

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
Display (the `next-display` shortcuts for each window).

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
doesn't have it. Windows coming back after an upgrade are the exception:
they come back as they were. Not an alert or a dialog of an app that
was started again (a new process: the old one is gone): it is placed
afresh, in the middle of the screen or over its parent, not where the
last one was, which may have been on a screen of another size
(`take_placement`); the same process reconnecting gets its back. A window asking to fill the screen (`fullscreen_request`) fills the
display it asks for, else the one it is on (`display_of`: most of it),
else the one the pointer is on; that display draws it alone
(`FullscreenSurface`), and the others leave it out
(`WindowMeta::fullscreen`) rather than draw its display-sized buffer
where it was, over their edge. With Preferences ▸ Appearance ▸ "Glow new
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

## Keyboard profiles (olo_protocol::shortcuts and app_keys, app_server keymap.rs, kit keyboard.rs)

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
the kit makes them the line's keys in a focused text field
(`mac_text_keys`).

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
(across the gap, or in line), which follow it live. They're paced like
the dragged window (`send_when_caught_up`): a new size goes out only
once they've answered the last, and each is moved only when it commits
(`tile_follower_drew`: the sides that don't move stay put, whatever size
it drew), and keeps catching up after the drag (`Follower`). A
neighbour linked on two sides (a corner drag) moves both. Sending one
per pointer motion queued configures faster than apps draw: the
neighbour lagged seconds behind and overlapped the window being dragged.
The drag stops where
one would get smaller than its app's minimum or `MIN_TILE`; at the end
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
(`focus_claim`, at its first commit), and one moved or sized by hand
zooms again where it is let go. An app's windows are always one group
there (`group_key`), so its others are tabs along the top. The rest are
covered (`WindowMeta::covered`, `mark_covered`, from the stacking order
just before drawing): not drawn and not under the pointer, but still in
the space at the same place and size, so showing one is only raising it,
and Deskbar, Twitcher and the overview have them as ever. Windows that
float (`floats`, as for auto-tiling) show over the one in front
(`keep_floating_over_focus`); a dialog (a window with a parent) is
covered with its parent, and an app's own panel (no parent: StyledEdit's
Find) while all its app's windows are, as BeOS's floating app feel did;
an app of only panels (DeskCalc) always shows. `WindowInfo::covered`
says which are covered, for agents.

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
programs portal dialogs and picture-in-picture (`floating_kind`). An
xdg-dialog alone doesn't make a window float (nor keep it out of its
app's tab group, `groups::is_dialog`): GTK 4 gives every toplevel one,
its main windows too, which then floated over the tiles and never
grouped. Floating windows stay in front of the tiles
(`keep_floating_above`). Anyone can float or tile a window by hand: the
window menu's Float Over the Tiles (only where auto-tiling is on),
⌃⌥⌘F (`float-window`), or `{"type":"set_floating","id":ID,"floating":true}`
(`null` leaves it to app_server again); `WindowInfo` says `tiled`,
`floating` and `float` (the hand's choice). A window that leaves for a
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
workspace, in whatever order the apps come back. Without saved trees
(from an app_server before them) they are made again from the windows'
saved parts (`auto_adopt`, `WindowMeta::restored_part`). An edge drag's
splits are set where the windows' least sizes let them be
(`auto_drag_ended`). A hand's choice to float or tile a window
(`Placement::floating`) comes back too.

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
refitted to its room, in case the Deskbar moved. Whatever a window
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
workspace and back comes back as it was. Only the
front window is drawn whole and takes input over its frame: the ones
behind it show just their tabs (`mark_behind`, before every frame,
`Decoration::behind`), so they never show around it while it is sized
smaller; a capture for an agent draws a window whole. Moving, resizing,
rolling up and tiling (one cell per group) act on the whole group; an
arranging command on a window behind brings it to the front first
(`front_of_group`). A
window leaving a group gets its own tab back and steps out by `CASCADE`
so it isn't hidden. Grouping is on for every app; the ones a person
turns off are `TilingSettings::ungrouped_apps` (app ids), toggled by
Group Windows in Deskbar's team menu and in Tracker's menu for an app
(`olo_kit::settings::groups_windows` / `set_groups_windows`). A group's
frame is never smaller than any of its windows can be (`least_frame`).

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
`Cx::begin_move`) starts from the pointer, not from that press; one
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
Esc, a click or any other key (which then does what it does) puts them
away; pressing Control twice again does too. Letters are by key position, as on a
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
boxes (egui's nesting) are `Group`s. With a menu open, only its items,
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
is a struct field and a BBox. `AI_MODELS` is OpenAI's current Codex
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
(`Request::SetBrightness`, perceptual 0–1). Brightness and volume keys are
taken by app_server; the brightness keys act on the display under the
pointer when its brightness can be set, else the built-in one.

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
`olo_screensavers::Module` that draws a whole frame into a rect with an
egui painter, so the same code runs full screen and in the preview monitor
of Preferences ▸ Screen Saver. Size things with `Frame::k()`, step motion
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

## Fractional scaling (app_server render.rs, shell/deco.rs; the kit)

A screen can be at 1, 1.25, 1.5, 1.75, 2, 2.5 or 3 (Preferences ▸
Displays; each is a whole number of 120ths, as wp-fractional-scale
sends it). At every one of them, nothing Olo draws is resampled, and
every line and edge is a whole device pixel. This is how modern
compositors and toolkits do it (KWin's pixel-grid snapping, Mutter,
Chrome, GTK 4.16 text), and how Olo does it:

- **Clients draw at the device size.** The kit asks app_server for
  wp-fractional-scale-v1 and wp-viewporter on every surface. It draws
  into a buffer of `round(size × scale)` pixels, with egui at that
  `pixels_per_point`, and sets the viewport's destination to the logical
  size. egui's screen is that buffer's size in points (`Surface::screen`),
  not the logical size, so what lines up with the right or bottom edge
  lands on a pixel whatever size the window is given (1153 points at
  1.5× is a 1730-pixel buffer, 1153⅓ points). app_server places each window on a whole device pixel
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
    `floor(scale)` (as the kit's `style::line_pixels`). The bevel's light
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
  - *The text caret* and egui's popup and tooltip outlines
    (`window_stroke`) are hairlines at each scale (`style::fit_scale`,
    which the platform calls every frame): a one-point stroke would be a
    pixel and a half at 1.5× and smear over two.
  - *An icon* is drawn at its texture's size from a whole pixel. A bitmap
    with no size of its own (ours come at 16, 24, 32 …, so 20 at 1.25×)
    is averaged down to exactly the device size once, when it loads
    (`icons::fitted`, as Chrome scales its icons), instead of being
    stretched by the GPU every frame.
  - *Viewer's pages* are drawn pixel for pixel once rendered for the
    current zoom and scale.
  - *An app that caches anything measured in pixels* keys it by the scale,
    as Terminal's cell metrics are (`Metrics::ppp`); a window can move to
    a display at another scale.
  - *At 1.25× and 1.75×* two points are 2.5 and 3.5 pixels. Edges are
    still crisp there (each is rounded), but spacing may differ by a
    pixel.
- **Text** is drawn by egui at the device size, with each galley on a
  whole pixel. It is hinted vertically only ("slight", as FreeType, GTK
  4.16 and Chrome do it; `SmoothHinting { light: true }`) and positioned
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

## The touchpad (app_server udev.rs, momentum.rs, touch_filter.rs)

[TOUCHPAD.md](TOUCHPAD.md) has the measurements and the reasoning.
Preferences ▸ Touchpad & Mouse has four tabs (Touchpad, Touch, Gestures, Mouse)
and a tuning profile (protocol `touchpad.rs`: `PROFILES`,
`apply_profile`; changing any tuning by hand makes it "custom"); the
pad and the profile it suits come from `Request::InputDevices`
(`DeviceKind::detect`). On MacBook pads (Apple MTP) app_server loads
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

Preferences ▸ Touchpad & Mouse goes into libinput (`configure_pointer`), with the
rest set as a Mac trackpad behaves: a two-finger tap is the secondary
click, no drag lock, no middle-click emulation. Scrolling has momentum, as
on a Mac (`momentum.rs`): when the fingers lift (libinput's scroll of 0)
while moving faster than 0.3 px/ms over the last 80 ms, the scroll glides
on, slowing by 0.997 a millisecond (`glide` stretches that: 2.0 glides
twice as far; `scroll_speed` scales finger scrolls before momentum
samples them); fingers resting again (a hold gesture), pointer motion, a
click, a swipe or pinch, or a wheel stop it.

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
between the reports either side, as Android resamples touches. The glide
goes out the same way, from where the fingers left off, so every frame
gets exactly one scroll of one frame's worth and the hand-over into the
glide is seamless. The lift's stop goes once the fingers' scroll is all
out. Olo's apps (`scroll::Toolkit::Olo`: a process with an
`olo-menus-PID.sock`) get the glide as a scroll of its own, source
`continuous` (`Pacer::sending_glide`, focus.rs), which the kit hands egui
as momentum, so it bounces off an edge as on a Mac; everything else sees
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
notch the same 48), so gets them ×0.4. Chromium is recognised before
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
clipboard; Copy to clipboard offers `image/png`. Settings are in `screenshot.json`. `--silent [--window [ID]]`
saves straight into the folder with no window: app_server runs it for ⌘⇧3
and Print Screen (the screen) and Alt+Print Screen (the shortcut's target
window by id: the active one, or the one the palette was opened over,
since the palette is active by the time it runs).
`--region` (⌘⇧4, region.rs) captures every output, covers each with an
exclusive overlay showing its picture dimmed, and saves the rectangle you
drag (its size in pixels follows the corner); Escape or a click cancels.
Saved with no window (`--silent`, `--region`), it prints the file's path
and puts it on the clipboard (`olo_kit::clipboard::hold`), staying until
something else is copied. ⌘⇧5 opens the app.

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
compositor, and the kit holds off re-executing it after an upgrade while
it shares (`App::busy`).

- **D-Bus** (`portal.rs`): CreateSession serves an
  `impl.portal.Session` at the handle the portal chose (Close drops its
  streams); SelectSources keeps the cursor mode (hidden or drawn in) and
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
- **The picker** (`picker.rs`): a fixed kit dialog over the asking
  window (`foreign_parent`): who asks (from the app id; "An app" when the
  portal can't tell, as for unsandboxed apps), the screens and the
  windows (not minimized; under Screens and Windows headings when both
  are offered, the windows scrolling past two rows) as `ThumbnailPicker`s
  (radio buttons to agents) of live thumbnails, and Cancel / Share
  (Enter, Escape, or a double click). Screens' wells take the first
  screen's shape (192×108 for 16:9), windows' are 160×100, all whole
  device pixels at every quarter scale; a worker keeps a capture of each,
  asks for a frame now and then (a screen twice a second, a window every
  two; app_server answers when it changed), and shrinks it to the well's
  size in device pixels for the scale it is drawn at, only ever from a
  whole frame (`Capture::frame_size`: not a buffer allocated for a new
  size that waits for its first, which is uninitialised memory).
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
- **Capture** (`capture.rs` in both): the portal is a client of
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
  resizes. Closing it stops its sessions, and the share ends two
  seconds later (`Capture::stopped`): app_server stops every session as
  it goes too (a reload, an upgrade), so a stop is only an end if the
  connection outlives it; else the share waits for app_server and
  carries on, its dmabufs made known to the new connection. While
  the screen is locked a window's share pauses on its last frame. Both
  protocols are kept from outside programs (wlproxy's list doesn't have
  them). A window share reconnects after a reload by its id, or one with
  the same app id and title (windows come back with new ids).

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
  SaveFile and SaveFiles as the kit's file panel (`file_panel`) in a
  window over the asking one, titled as the app asks, starting in its
  `current_folder` (a save's `current_file`'s), proposing its
  `current_name`. The app's filters (globs and MIME types; `current_filter`
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

## DeskCalc (apps/deskcalc)

An expression line over a keypad that scales with the window: R5's 5×4
keys, Haiku's scientific rows above them, or none; the context menu
switches keypads, angle units and copies/pastes. `expr.rs` is the whole
calculator (lexer, recursive descent, `format` for 15 significant digits)
and is unit-tested; keep new functions in `FUNCTIONS` and on the keypad
together. After =, a digit starts afresh and an operator carries on.
Settings, the last expression and history are in `deskcalc.json`.

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

`doc.rs` is the model and is unit-tested. The window is the kit's
`editor::Editor` (so a document of any length scrolls and types at full
speed), and the `Doc` is its `Document` (`Styled` in main.rs, which also
names each style's font): every edit is `Doc::edit`, which moves the
runs: inserted text takes the pending typing style, the style of what it
replaced, or the style before it, and pasting what was copied here
brings its styles back. Undo is our own (snapshots, typing and deleting
coalesced within 2 s); ⌘Z/⌘⇧Z/⌘Y are consumed before the editor sees
them, and what they change goes to the editor with `replaced`. Replace
All splices each match so the styles between them survive. A read-only
document's Font and Document menus are disabled (`Menu::all_enabled`):
its styles are saved with it.

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
GPU memory from DRM fdinfo), Sensors (the battery's health, cycles and
capacity; every temperature, power and fan sensor), System (the
machine, pressure, and the kernel's tunables, which can be set) and
Graphs (Haiku's ActivityMonitor: graphs of any sources, each scaled to
its own range, from the context menu). ⌘1 to ⌘9 are the sections
(`app_keys`: `activity.overview` and on). Click any graph to look back:
the Overview's last card says what was busiest at that moment (top CPU
and disk processes are kept for each sample, half an hour of them).

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

## The supervisor (apps/supervisor, Preferences ▸ Performance)

Keeps the desktop responsive; off by default. `olo-supervisor` is one of
the real session's services (`REAL_SHELL`): it waits, following
`performance.json` (`olo_protocol::performance::PerformanceSettings`),
until turned on in Preferences ▸ Performance, by Deskbar's gauge (a
click; its menu has the findings, Show in Activity Monitor and
Performance Settings…) or `olo-supervisor --on`. Then every two seconds
(about 6 ms on its own thread; each command line read once a process)
it looks at the person's own processes in /proc (`sample.rs`) and:

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
the running system (`Volume::system`, `Disk::system`). The forms are
alert windows with fields (`alert_frame` + `alert_text` +
`alert_buttons`) that resize to what they show. Agents: `--list` prints
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
go into it, the middle (or Up, ⌘↑, Backspace) to go back up.

The start screen lists Home and each disk file system once
(`volumes.rs`, from /proc/self/mountinfo; statvfs for size and free
space). Scanning (`scan.rs`) is a worker thread that adds each folder to
a shared `Tree` in one write, so the chart and list grow as it goes
(the window sorts while it runs; the tree is sorted once at the end).
Sizes are allocated blocks, hard links count once, symlinks are never
followed, and it stays on one file system: `volumes::Mounts` crosses a
mount point only for another part of the same file system (a btrfs
`/home` under `/`), never `/proc`, `/tmp`, other disks or a bind mount
of what it already covers. Moving to the Trash (`olo_kit::trash`) and
deleting run on a thread too; the tree drops what went
(`Tree::remove`). `save_state` keeps the scanned folder, the folder
shown and the Collector; after a re-exec it scans again and goes back.
Test against a fixture home (`fallocate` files under /var/tmp) with
`tsession run NAME HOME=… target/debug/olo-disk-usage DIR`.

The freedesktop Trash helpers Tracker and DiskUsage share are
`olo_kit::trash` (olo_xdg): `trash_slot` reserves a name and writes the
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
(A Terminal from before olo-termd hands its shells' PTYs to its new
self, the old way; the new one passes them on to olo-termd, ADOPT.)
When the desktop session ends (its pid file, watched with a pidfd) or on
SIGTERM, olo-termd hangs up every shell and exits; it also exits when
it has no shells and no Terminal for 3 s, or when its runtime directory
has gone. If its socket is deleted from under it, it listens again (so a
Terminal can always find its shells); it only removes the socket while
the file there is still its own. Each screen is kept twice (in
olo-termd and in Terminal), so a full scrollback costs about twice
what it used to.

**Restoring.** Terminal keeps its windows on disk as they change
(`remember`: every 2 s if anything differs, and before it quits), in
`$XDG_STATE_HOME/olo/terminal/windows.json`: each window's tabs in
order, which one shows, its size, group, Tile All Tabs (and its chosen groups), and each tab's
folder. A Terminal that starts afresh (not an upgrade, which has
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
underlines it and ⌘-click opens it (the profile's command key: Ctrl with
the Windows keys, `egui::Modifiers::command`, which the kit sets for a
raw-keys window's clicks and frames as for its keys): a URL in the
default browser, a file or folder in Tracker (`olo_kit::tracker::open`, a
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
read it as `selected`): clicking one tiles only the chosen groups, one
alone in a full grid, several a row each; clicking it again unchooses
it, and choosing every group is the same as none (every tab). The
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
the strip's first row centred on it) with its icon and title, in its
group's colour as the strip shows it (its ground tinted with it and a
2-point line of it across the top) so it's plain which pane is in which
group; panes after the first mark themselves `layout_review::own_grid`,
since dividing the window puts them level with the strip only by
chance; and the
panes not typed into are dimmed so the one that is stands out, its head
lit like a window's tab in front; the strip
is `.closed()` then, since no single tab owns the content.
Clicking a pane makes it the one typed into, its head marked with the group's
colour (or the focus blue) as the strip marks it, and the strip follows.
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
every 600 ms: if the shell's foreground process is Claude Code (its
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

**Coding agents' tab titles** (`src/agent_title.rs`; Preferences ▸
Terminal ▸ Coding Agents, off by default). A Claude Code or Codex in a
tab gets a tool, `set_tab_title`, and is asked to keep the tab titled
with what it is working on: the hook above (installed while either
setting is on; the file says which, `Snapshot::groups`/`titles`) prints
the instructions at `SessionStart` and a one-line reminder with each
prompt. The tool is an MCP server, `olo-terminal --mcp` (JSON-RPC 2.0, a
line each way on stdin and stdout: `initialize`, `tools/list`,
`tools/call`), which both agents run outside their sandboxes (Codex's
sandboxed shell can't reach Terminal's socket). On, Terminal names it in
Claude Code's global `.claude.json` (`mcpServers.olo-terminal`, in
`CLAUDE_CONFIG_DIR` else the home folder; Claude Code rewrites that file
now and then, so it is read just before and checked after), allows the
tool in its `settings.json` (`mcp__olo-terminal__set_tab_title`, so it
isn't asked each time), and in Codex's `config.toml`
(`[mcp_servers.olo-terminal]`, through `toml_edit`, the rest of the
file as it was); off, it takes them out. Agents see it from their next
start (`claude mcp list` and `codex mcp list` show it). The server finds
its tab as the hook does (Claude Code starts it under the tab's shell;
Codex's daemon by the folder the agent passes as `cwd`, else its own)
and hands the title to that tab's Terminal on the tab's display
(`olo-terminal --set-tab-title SHELL TITLE` through the single-instance
socket, `instance::forward_on`). The tab shows it (`Tab::agent_title`,
one line, at most 60 characters) ahead of the program's own title until
the agent leaves the tab or sets an empty one; it is saved with the
window, and after a restart kept only if the tab's agent is seen again
within 5 s.

**Repositories** (`src/git.rs`, `src/repos.rs`), in process through
libgit2 (git2, vendored; no `git` or `gh`). A thread (`git::Watcher`)
looks at each tab's folder (its foreground program's, else its shell's)
every second and reads only what changed: the folder, the worktree's
`HEAD` and the repository's `config` (a folder in no repository is
looked at again every 5 s, for a `git init`); it reports the tab's
repository (`git::Repo`: name, main worktree, the worktree it's in,
linked or not, branch, GitHub remotes) and, for each repository a tab is
in, its worktrees and local branches when `worktrees/*/HEAD`,
`refs/heads` or `packed-refs` change.
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
when it is back (`set_text_size`, `Cx::is_shown`), rather than jump
then. The menus read ⌘N, ⌘T, ⌘+, as pressed. ⌥←/⌥→ send ESC b / ESC f
(a word back/forward in readline, zsh and fish), except to a program
using the kitty keyboard protocol (Claude Code), which gets the
protocol's `CSI 1;3 D` / `C` and reads ESC b as Escape then b. (The
arrows must stay legacy `CSI 1;mod X` keys there: egui names them
⏴⏵⏶⏷, so `view::kitty_code` takes only ASCII names as codepoints.)

**Colours** (`src/colors.rs`, Settings ▸ Colors). Match Appearance,
Always Light or Always Dark picks which of two schemes shows; Light
Colors offers Olo Light, Catppuccin Latte and Catppuccin Frappé, Dark
Colors Olo Dark, Catppuccin Macchiato and Mocha (`SchemeId::LIGHT` /
`DARK`). Frappé, the lightest of Catppuccin's dark flavours, is the
light default and Macchiato the dark one (`SchemeId::light` / `dark`).
A Catppuccin scheme draws as its own ports do (`Scheme::catppuccin`:
Text on Base, Rosewater cursor, its 16 ANSI colours); the choices are
`Settings::light_colors` / `dark_colors`.

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
window. Whatever olo-sudo can't help with (`-n`, `-S`, `-A`, `-k`, `-K`,
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
(`Cx::stay_resident`) and opens a BAlert-style window per request
(`widgets::alert_frame`). Proving identity runs polkit's setuid
`polkit-agent-helper-1` (the only thing polkitd accepts an answer from):
the cookie goes on its stdin, and its PAM conversation (`PAM_TEXT_INFO`
from fprintd for Touch ID, then `PAM_PROMPT_ECHO_OFF` for the password)
is relayed to the window; a failure starts it again for another try.

Test it with `tsession run NAME olo-polkit-agent --self-test ACTION`:
it serves only its own process and asks polkitd about ACTION with
`CheckAuthorization` (which changes nothing), printing the outcome to
its log. **Cancel; never type the user's password, and only check
actions, never perform them.** `OLO_POLKIT_HELPER` points at a fake
helper for unit tests.

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
keyring dismisses the request instead of hanging.

Test it on a private bus with a throwaway daemon, never the session's:
start a bus (`dbus-broker-launch` with a listening socket handed over as
`LISTEN_FDS`, a permissive config) and `gnome-keyring-daemon --foreground
--components=secrets` with its own `HOME` and `XDG_RUNTIME_DIR`; then
`KEYRING_TEST_BUS=unix:path=… cargo test -p olo-system real_daemon --
--ignored` (makes a keyring, wrong then right password, cancel), or
`olo-polkit-agent --keyring-test unix:path=…` in a headless session and
a Secret Service client calling `Unlock` and `Prompt`.

## Notifications (apps/notifications)

Olo's own apps notify through the kit: `olo_kit::notify::Notifier::
new(cx.proxy(), "Tracker", "olo-tracker")`, then `show(None,
Note::new("Copied").body(…).action("default", "Show"))` → a `NoteId`;
`show(Some(id), …)` updates it in place (`Note::progress(title, 0.4)`
for a job's bar: transient and silent), `close(id)`. Kinds are
`NoteKind::{Information, Important, Error, Progress}`. The bus is used
on the notifier's own thread; what the user does comes back to
`App::message` as `NoteEvent::Action { id, key, token }` (bring a window
forward with `cx.activate_with(win, token)`) or `NoteEvent::Closed`.
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
  stop sign (`look::paint_stop`: `style::stop_mark` set level with the
  capitals by `style::cap_middle`), as do its rows in the history; progress
  shows a `StatusBar`; links in the body open with `open::open`. A card
  is laid out by look.rs; without a picture the text takes the width,
  the group's title row already showing whose it is: `paint_mark` draws
  the named icon (ours for common theme names, `named_icons`), the app's
  desktop-entry icon, else a monogram for a site or an app with an accent,
  else a generic one. Finding an app's desktop entry can read every one
  on the system (milliseconds each time; it held the History's Cards to
  30 fps), so `app_icons` looks each app up once, on a worker
  (`warm_app_icons`: the history's apps at start, a note's as it comes),
  and keeps the answer. Action buttons share the width when they must.
  Banner `seconds` 0 ("Never" in Preferences) shows only errors.
  A click on a card runs its `default` action: the card asks the kit
  for an activation token from that click (`Cx::activation_token`) and
  sends `ActivationToken`, `ActionInvoked`, then `NotificationClosed`, so
  the app may bring its window forward (app_server lets a token one of
  our programs asked for, with an input serial, raise another app's
  window). With no default action the click brings the app's window
  forward itself, over IPC, by desktop entry or name. Action buttons
  along the bottom; a reply field and Send when it can be answered (the
  layer takes the keyboard on demand only while one shows,
  `Cx::set_keyboard`); the close box dismisses. While the pointer is over
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
  `strength` (`multiply_opacity`), the newest strongest, back to full
  while selected or under the pointer. *Cards* packs cards of different
  sizes with `olo_kit::masonry`: columns of at least 176 px; a card
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
  (`save_state`).
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
  Now to gone. The service re-reads the
  file within a second of a change (Preferences ▸ Notifications and
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
  signal. Flatpak apps reach us through the portal: `olo-portals.conf`
  leaves Notification to xdg-desktop-portal-gtk, which sends a plain
  `Notify` (no app name: we take the `desktop-entry` hint's) and runs the
  app's action itself when we signal `ActionInvoked`.
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

⌘Space (`Shortcut::Palette`, the profile's: ⊞S on Windows, Alt+Space
on BeOS; Find moved to ⌥⌘Space) or a tap of the Super key (if chosen in
the keyboard profile; Windows' does) has app_server start
`olo-palette --toggle`; the running copy (it stays resident, found
through `instance`) takes the toggle, so it opens at once from the second
time on. It opens a full-screen transparent overlay layer that takes the
keyboard, on the display in use (the focused window's, else the
pointer's), with the BeOS panel a fifth of the way down (over a soft
shadow on device pixels, `style::soft_shadow`, as the shortcuts sheet's;
the query's capitals on the field's middle, `style::text_origin`): a
click outside the panel, Escape or ⌘Space again
put it away.

* **Before opening**, a worker asks app_server for the windows and the
  settings, so the window in front is known before the overlay takes the
  keyboard (app_server's focus is then the overlay). Shortcuts are done
  to that window through `Request::RunShortcut { id, window }`.
* **Sources** (`sources.rs`): applications (ours, then desktop entries),
  preference panels, light/dark appearance, app_server's shortcuts with
  their current keys (`keys_for`, shown as on a Mac, so "tile" lists every
  tiling shortcut and its keys), open windows, the front app's menu
  commands with their keys (`olo_kit::commands`, asked of the process
  that owns the front window; disabled items are shown dimmed), Passwords'
  items (`olo_keychain::Request::Search` while unlocked; choosing one
  is `Request::Show`), files under home by name (`sources::Index`: an
  `ignore` walk on start, at most 400,000, then each folder followed,
  `olo_kit::watch`, and only one that changed listed again, so a new file
  is found at once; past an eighth of the kernel's inotify watches it is
  walked again when asked and five minutes old instead; matched on their
  own thread for the newest query only, which sleeps until a query or a
  change), sums (`calc.rs`; Return copies)
  and, last, a web search (Google) in the web browser.
* **Ranking** (`rank.rs`): every word typed must match the title (start,
  word start, anywhere) or the item's keywords, or as initials ("nw");
  then by kind (apps before files) and by how often and how lately each
  thing was chosen (`~/.config/olo/palette.json`). An open window is
  also found by its app's name and comes just before the app itself
  (switching beats opening another), the one in front last. Nothing
  typed lists what was chosen lately.
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
  next/previous display (with more than one), Open in Tracker (when the window's program says
  what it shows: `commands::path`, asked of its pid with its
  `WindowInfo::surface`), Send to Back (BeOS's right-click on the tab,
  `Request::SendToBack`) and Close. Its items are the shortcuts', done
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

## Other toolkits' programs (app_server toolkits.rs, state.rs; olo_theme::toolkit)

Programs not built on the kit take Olo's look as far as their toolkit
allows. `toolkits.rs` publishes the palette where GTK 3, GTK 4,
libadwaita and Chrome read it (GSettings through dconf, the Olo and
Olo-dark GTK themes, GTK 4's user stylesheet, BeOS's close-left,
zoom-right title buttons); see its module comment. Fonts are hinted as
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
else reads the name: xdg-desktop-portal still takes `olo-portals.conf`
(the first name with a file wins); `open.rs` reads
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

## Agent Screens (apps/screen)

Agents get screens of their own: `olo-screen start [NAME] [--size WxH]
[--scale N] [--sound]` starts a headless Olo session (`olo-session
--headless`, in its own process group), a whole desktop with its own
Deskbar, windows, pointer and keyboard focus, so nothing an agent does
there moves the person's pointer or takes their focus. It runs the
person's programs and files (their HOME), but with app_server settings
of its own (`OLO_APP_SERVER_SETTINGS`, a copy of the person's at 1×
unless asked, with no lock, screen saver, display sleep or system
sleep), silent unless `--sound` (`PULSE_SERVER` pointed at nothing), and
without `DISPLAY` (X programs would show on the person's screen). Screens
are listed in `$XDG_RUNTIME_DIR/olo-screens/NAME.json` (the session's
pid, its sockets, the size); `screens.rs` checks the pid is still that
session before trusting an entry. The rest is the screen's own app_server
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
aliases) and `type` (US keyboard), all as `Request::Synthetic`, in the
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
history's folder (`olo_kit::watch`) and reads only what was added
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
`picture_well`), what the action was and what came of it under it
(what it matched, the program's pid, the window in front after, or why
it failed), and plays it back in time with them (Play/Pause, 1× to 8×,
◀ ▶ action by action among those shown; stretches of more than 3 s with
nothing in them skipped), each click a ring where it went and the
newest key or text a caption under the picture. At the newest action it
follows the screen (Live, or Latest for a stopped one, pushed while it
does): each new action is chosen as it comes and brought into view
once, so the list scrolls freely otherwise, and rings and captions fade
on the clock. Narrow, the scrubber goes over the buttons. Frames are
built on a worker, the latest asked for winning.
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
kit's `Sidebar`) has every screen running and every one stopped with a
history kept, under Running and Stopped, by last use (`viewer::rows`,
read on a worker; the chosen one's as it acts), each with how long ago,
and New Screen under them. The chosen screen's header names it and says
how it is (running and its size, up for how long, its actions, the
last how long ago), with Export Video… and Stop Screen… (Clear
History… for a stopped one). Stats (`stats.rs`) are the kit's dashboard
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
rows carry AccessKit `row_index`/`row_count` (`UiNode::row`/`rows`,
printed `row 13/61`); egui's "toggled" on a row, tab or column header is
reported as `selected`. `focus`/`close`/`minimize`/`zoom` are the window
requests; `resize` and `place` are `Request::Place` (a frame, as
`set_frame` gives tiles). `look` writes beside the logs when given no
file, and `--crop` cuts an RGBA picture down in process (png crate).

The person's side is the window (`olo-screen`, or `olo-screen
watch NAME`): the screens in a sidebar and the chosen one's Live tab,
about eight frames a second with the agent's pointer drawn in and its
latest action under it (a worker asks for `format: "rgba"` pictures and
reads them back, only while Live shows; the window never waits on a
screen). Take Control (⌘K) steps in: the person's pointer,
wheel and keys over the picture go to that screen through another worker
(keys by their evdev codes, Ctrl, Shift and Alt pressed and let go as
each key event says), with a ring in the keyboard-navigation colour
round the picture, until Stop Controlling. New Screen starts one; Stop
Screen… ends one after asking. A viewer running on an agent screen
leaves that screen out of its list. While any screen runs (other than
the one a Deskbar is on), Deskbar's tray shows a monitor with a pointer,
and their count when there are several (`status::agent_screens` reads
the same listing as a screen's file comes or goes, `olo_kit::watch`, and
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

## Google Chrome (apps/chrome)

`olo-chrome` is Google Chrome in Olo's applications (`OUR_APPS`).
With Chrome installed it execs Chrome's own launcher
(`~/.local/share/olo/chrome/current/google-chrome`) with its arguments,
adding `--ozone-platform=wayland` under Wayland (Chrome otherwise goes by
`XDG_SESSION_TYPE` and falls back to X11). Without it, it offers to
download it (`install.rs`, on a worker thread, cancellable): Google's rpm
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
`manifest.json` whenever the extension changes**. The worker reloads on
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
size it will be (`olo_kit::files::set_expected_size`, the
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

## Log Viewer (apps/logs)

`olo-logs` shows every Olo program's log in one place: what they
write to stderr goes to the user's journal (through the session's
journal stream), and Log Viewer reads that journal natively. Kit apps
log `warn,usvg=error,olo=info` unless `RUST_LOG` says otherwise
(`olo_kit::run`; tsession starts clients with `RUST_LOG=warn`): Olo's
own crates keep their info for what is rare and worth knowing after
the fact (a reconnect, an upgrade, why a close was asked about: the
Terminal's `asking before closing` names the program and where the
close came from, app_server's `asked to close a window` and `close box
clicked` the window and who), dependencies only their warnings.

- journal.rs: systemd's journal file format (JOURNAL_FILE_FORMAT.md): the
  header, entries through the chain of entry arrays, fields in data
  objects (zstd, xz or lz4; the regular and the compact layout). Data
  objects are cached by offset (most are shared between entries). A
  file still being written is read on from where it was left, and
  across journald rotating it (the open file is finished, then the new
  one opened). `user_files(uid)` finds `user-UID.journal` and its
  archives in /var/log/journal (or /run/log/journal), oldest first.
- reader.rs: on a worker thread, every file in batches, then the active
  one read on as it changes (`olo_kit::watch`: journald truncates the
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
  `olo_kit::search`): `since:` (10m, 2h, 1d, today, boot, all; since
  startup if not said), `level:` (the least: debug, info, warning, error;
  info if not said), `program:` (several, any of them, matched by their
  start while one is typed; `all` for every program; none: Olo's),
  `-program:` (left out), `pid:`, and words, phrases and `-words` in the
  message or the program.
- highlight.rs: syntax highlighting as spans: the target `tracing` puts
  first, `key=value` keys, strings, numbers, paths and URLs, type names,
  trouble words; the list and the detail pane colour them from the
  palette.
- main.rs: the kit's `SearchField` and a `FilterBar` (Program, with
  each program's lines in the query's time and Olo's first; Level; Time)
  above a `ColumnListView`, in one row when the window has room for the
  field, else the menus under it (their text over the list's first
  column); what the words find is marked in the messages
  (`search::mark` over the syntax colours; rows' galleys kept by the
  words too). The selected line in full (highlighted, selectable)
  below; Live keeps the list at its end as lines arrive; Show in Context
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

`olo-lock` uses the kit's `cx.lock_session()` (`SessionLock::Locked`
gives one window per output, the first is primary; `cx.unlock_session()`
ends it) and checks the password with `olo_system::auth` (PAM via
libpam, on a thread) while listening for Touch ID with
`olo_system::fingerprint`. Debug builds take `OLO_PAM_CONFDIR` +
`OLO_PAM_SERVICE` (passed through by tsession), so tests use a private
pam_exec service and never a real password:

```sh
printf 'auth required pam_exec.so expose_authtok quiet /path/check.sh\naccount required pam_permit.so\n' > $dir/exec
OLO_PAM_CONFDIR=$dir OLO_PAM_SERVICE=exec tools/dev/tsession start lk --light
tools/dev/tsession ipc lk '{"type":"lock"}'; tools/dev/tsession type lk secret; tools/dev/tsession key lk 28
```

## Passwords (apps/passwords, crates/onepassword)

**Searching**: the toolbar's `SearchField` and a `FilterBar` over the
list (Tag, Has) share one query (`olo_kit::search`). Words find an item
by its title, username, websites, tags, kind, and the text of its fields
that aren't secret and its notes; with words, the best come first
(title 4, username and site 3, tags 2, the rest 1), else by title; what
was found is marked in each row, whose second line is the username (else
the site). Filters: `type:` (login, secure-note…), `vault:`, `tag:`,
`site:`, `user:`, and `has:` otp, password, website, files, reused (its
password is another item's too), weak (under 12 characters, or of one
kind). The vault worker sends a `Searchable` for every item with the
list (`store::index`, again after each change): the username, the
non-secret fields' names and values and the notes, and flags; never a
password, code, card number or key, and of a password only whether it
is weak or reused. So searching finds usernames of items never opened.

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
Generator window, and File/Edit/Item/View menus (⌘C username, ⇧⌘C password,
⇧⌘O one-time code). Secrets go to the clipboard with `copy_secret` and
leave after `clear_seconds`. A field (or website) drags out as text onto
another app's field (`drag_field`: a one-time password's current code;
the picture shows the field's name, never the value). It opens locked and locks after
`lock_minutes` idle, and when the screen locks if `lock_with_screen`
(`lock.rs`): Touch ID through fprintd, claimed only
while the locked window has the keyboard, or the account password through
PAM. Those, and `auto_sync`, are Preferences ▸ Security's
(`Settings::security.passwords`, followed live through
`olo_kit::settings::watch`; the screen locking comes from
`settings::watch_power`, `PowerStatus::locked`; View ▸ Lock After writes
the same setting with `settings::update`). Its own settings (accounts,
the generator, the window size) live in `passwords.json`.

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
  itself. Up: 1Password's copies of items changed here
  are fetched to compare. The plan is the store's `Change`s, what to send,
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
`tools/dev/private-keyring start` (busd plus its own gnome-keyring; needs
`cargo install busd --root /tmp/olo-dev/busd`), then
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

`op` asks over the keychain socket (`Request::Cli { query: CliQuery }` →
`Response::Cli { json }`, in 1Password's JSON shapes), starting Passwords
if it isn't running; the shapes, field types and `op://vault/item/[section/]field[?attribute=…]`
references are `apps/passwords/src/cli.rs`. Account questions (`op
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
(SO_PEERCRED + `/proc/PID/exe`). `olo_keychain::call` blocks, so call
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
- **Upgrades of the pool itself** (`drain.rs`): the kit tells the app a
  new olo-pool is installed (`App::upgrade_due`) and becomes it once
  `App::busy` is false. Meanwhile the pool takes no new connections
  (the proxy's accept loop and the gateway's axum server stop, the
  sockets left listening, so a new connection waits in the kernel's
  queue as if the pool were only slow), each request under way runs to
  its end (`Underway`, held by its answer's body until that has gone:
  `Counted`), each connection closes once its request has (hyper's
  graceful shutdown; a client opens another for its next, which waits
  too), and a tunnel counts as under way while bytes cross it (10 s
  idle and it doesn't). Then the listening sockets (copies kept at bind,
  `Drain::keep`) lose their close-on-exec and are named in
  `OLO_POOL_LISTENERS` (`PORT=FD`), and the new version takes them over
  (`drain::listen`) and answers what waited. Waiting gives up after 10
  minutes; an upgrade that still hasn't come a minute later (the new
  binary wouldn't start) ends the drain and connections are taken again
  (`Drain::end`). `/pool/status` says so (`drain`). Tested against a
  fake provider (`tests/drain.rs`) and end to end: a stream under way
  finished whole, and a client that connected mid-drain was answered by
  the new version on the same pid.
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
