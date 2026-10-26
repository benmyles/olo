# Hacking on Tabula

Tabula is a Wayland desktop environment in Rust that captures the look and
feel of BeOS R5, with deliberate modern polish. **North star: everything is
pixel-perfect and beautiful, in light, dark and night modes, at 2× (and any
other) scale.** Every deviation from BeOS should be an intentional
improvement, never an accident.

## The responsiveness rule (applies to everything)

**Nothing may ever block the UI.** BeOS was famous for staying responsive
under load; Tabula must be at least as good. This is a hard requirement for
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
   the desktop.
7. **Stay current without polling.** What is on screen reflects the system
   as it changes: watch files with inotify (Tracker's `watch.rs`: one thread,
   changed items stat'ed off the UI thread, bursts gathered for ~30 ms,
   a full relist on queue overflow) and the mount table with `POLLPRI` on
   `/proc/self/mountinfo`. No timers that re-read the disk to look for
   changes.

## Layout

| Path | What |
|---|---|
| `crates/app_server` | The compositor (Smithay, forked from Anvil). Draws the BeOS window decorator (yellow tabs), manages windows/workspaces, serves IPC. |
| `crates/session` | `tabula-session`: starts and supervises app_server, Deskbar and the desktop; in-place reload and upgrade (see below). |
| `crates/protocol` | JSON-lines IPC between app_server and our apps (`Client`, `Request`, `Reply`, `Event`, `Settings`). |
| `crates/theme` | `Palette` (light/dark), BeOS tint helpers, decorator metrics, font lists. |
| `crates/kit` | The Interface Kit: Wayland client platform, GPU rendering (egui on wgpu/Vulkan), menus, BeOS widgets, icons, `.desktop` entries. **All apps are built on it.** |
| `crates/xdg` | `tabula-xdg`: freedesktop plumbing shared by app_server and the kit (re-exported there): data dirs, `.desktop` entries, icon lookup/decoding (`load_icon`), our own apps (`apps::our_app`), MIME types (`mime::for_path`), opening files and URLs with the right app (`open::open`, `open::handler_cached`: never `xdg-open`), `find_program`, `spawn_detached`. No egui. |
| `crates/system` | `tabula-system`: the system over its own protocols, in process: logind (`power`: reboot, power off, suspend, inhibitor locks), NetworkManager (`network::wifi`), the sound server (`audio`: volume, mute, `watch` change feed), dconf writes, the D-Bus/systemd activation environment. All blocking: call from worker threads. |
| `crates/preview` | `tabula-preview`: native previews of every file kind (text, images, PDF, audio, video, archives, office documents, fonts). Used by Tracker's Quick Look (`Preview::new`, images capped at 3200 px) and the Viewer (`Preview::new_zoomable`: full resolution up to the GPU texture limit, zoom and pan, `zoom` / `set_image_scale`). Images carry halved copies (`load::halvings`) because egui-wgpu makes no mipmaps. Video (`video.rs`, H.264 in mp4 or Matroska, with `media.playable` set): a worker demuxes and decodes with OpenH264 up to six frames ahead, giving each picture the earliest outstanding presentation time (OpenH264 returns display order); `Video::tick` shows frames against a clock that follows the sound's reported position when an `audio::Player` is playing it and runs alone otherwise (no sound server in tests, silent files). Seeks go to the key frame before the target (Matroska: scans back for one, further each time) and drop pictures before it; a paused seek still shows its frame. `TABULA_SAMPLES=dir cargo test --release -p tabula-preview video -- --ignored` checks seeking on test.mp4/mkv (one key frame) and gop.mp4/mkv (one a second), made with ffmpeg's testsrc2. Audio decodes with symphonia plus `opus.rs` (opus-decoder, OpusHead parsing and pre-skip trim) for Ogg Opus. |
| `apps/<name>` | One crate per application, binary named `tabula-<name>`. Picked up by the workspace glob. |
| `data/icons` | Icon set, pre-scaled to device sizes (`data/icons/<px>/<name>.png`). |
| `data/session` | `tabula.desktop` for the login screen (installed by `tools/install-session.sh`). |
| `tools/` | `install-session.sh` (release build → `/usr/local`), `dev/tsession` (headless UI testing), `genicon.py`/`genicons.sh`/`mkicons.py`/`iconsheet.py` (icons, see [ICONS.md](ICONS.md)). |

Build: `cargo build -p tabula-<app>` (plus `cargo build --bin tabula-app-server` once, for headless testing; as root set `CARGO_HOME=/home/ben/.cargo` to share the registry) (repo is owned by `ben`; when running as
root use `sudo -u ben bash -lc 'cd … && cargo build …'` in the main checkout).

## Writing an app

```rust
use tabula_kit::{App, Cx, Menu, WindowId, WindowOptions, egui, widgets::*, metrics as m, style};

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
    tabula_kit::run("tabula.myapp", |cx| {        // app id: tabula.<name>
        cx.open_window(WindowOptions::new("My App", 480, 320).min_size(320, 200));
        MyApp { /* … */ }
    });
}
```

* **`App`**: `ui` (draw a window below its menu bar), `menu_bar`, `command`
  (menu item or ⌘-shortcut), `close_requested`, `message` (from background
  threads via `cx.proxy().send(msg)`), `theme_changed`, `files_dropped`,
  `save_state` (see "In-place reload").
* **`Cx`**: `open_window`, `close_window`, `set_title`, `resize`, `redraw`,
  `activate` (bring a window forward and focus it, restoring it if it is
  minimized or on another workspace — via xdg-activation; use it when a
  forwarded launch asks for a window that is already open), `copy` (text
  to the clipboard), `copy_data` (any MIME types, e.g. `image/png`; the
  data is written to pasters from a thread), `paste` (reads the clipboard
  off-thread and delivers an `egui::Event::Paste`), `quit`, `stay_resident` (keep running with no windows), `palette`,
  `proxy`, `window_size`, `restored_state`. Worker threads may call
  `ctx.request_repaint()` (or `_after`) on a window's context: the kit
  wakes its loop through egui's repaint callback and redraws that window.
  There is no clipboard manager,
  so an app that quits (or closes its last window) while it holds the
  clipboard stays running, windowless, until something else is copied.
* **Single instance** (`tabula_kit::instance`): call `instance::forward(app_id,
  &args)` first in `main` (returns true if a running copy took the
  arguments; exit then), and `instance::listen(app_id, cx.proxy())` in the
  init closure; forwarded launches arrive in `App::message` as an
  `instance::Launch { args, cwd }`.
* **Fixed-size windows**: `WindowOptions::new(..).min_size(w, h).max_size(w, h)`
  (preference panels, alerts).
* **Raw keys** (`WindowOptions::raw_keys()`): for windows that consume the
  keyboard themselves (Terminal). ⌘-letter combinations reach the app as
  key events instead of menu shortcuts or clipboard events (⌘⇧C/X/V still
  become Copy/Cut/Paste), and Alt+text is delivered as text.
* Background work goes on threads; post results with `Proxy::send` and
  handle them in `App::message` (then `cx.redraw(window)`).
* Periodic refresh (clocks, graphs): `ui.ctx().request_repaint_after(duration)`.
  Never use it to look for changes on disk; watch them (rule 7 above).
* Layer-shell surfaces (Deskbar, desktop): `WindowKind::Layer(LayerOptions{…})`;
  `LayerOptions::output` puts one on a named output (`cx.outputs()` lists
  them; the screen saver opens one per output). `LayerOptions::keyboard`
  is `Keyboard::None`, `OnDemand` (focus on click: the desktop) or
  `Exclusive` (every key while it is up: pickers such as Screenshot's
  region overlay).
* `ui.ctx().set_cursor_icon(CursorIcon::None)` hides the pointer over a
  window (screen savers). Other `CursorIcon`s go to app_server by name
  over cursor-shape-v1 (sctk does it), and app_server draws its own
  cursor for them; apps never ship cursor images.
* Staying awake: `cx.keep_awake(window, true)` holds a zwp_idle_inhibit
  inhibitor on the window (no dimming, screen saver, lock or sleep while it
  is visible) until `false` or the window closes; app_server counts these
  in `PowerStatus::app_inhibitors`. The Viewer and Quick Look hold one
  while `Preview::playing_video()`.
* Session lock (the lock screen only): `cx.lock_session()`, then
  `SessionLock::Locked(windows)` in `App::message` (one window per output,
  the first is primary; kept up to date as outputs come and go) and
  `cx.unlock_session()`. See "The lock screen".

### Widgets (`tabula_kit::widgets`)

`Button` (`.default(true)` for the Enter button), `CheckBox`, `RadioButton`,
`Button::pushed(true)` for a toolbar toggle that stays down,
`TextControl` (`.label("Name:").divider(x)`), `TextView`, `Slider`
(`.label().value_text().limits()`), `MenuField` (pop-up choice), `BBox`
(etched group box with label), `separator`, `StatusBar` (progress),
`ScrollView` (`show`, `show_rows` for virtualised content, `show_in(ui,
rect, |ui, viewport|)` for content you lay out yourself, `reveal`,
`ScrollView::scroll_to(ctx, id, offset)`, `.auto_hide(true)` to drop the
bar when content fits; `SCROLL_BAR` is its width),
`ColumnListView` + `Column` + `ListState` (sortable, resizable,
multi-select, context clicks; `.reveal(row)` scrolls a row selected in code
into view), `TabView`, `color_well`, `heading`,
`form_label`, `window_content`, `button_row`, `context_menu`,
`popup_menu_at` / `popup_result`, `InlineEdit` (Tracker-style rename in
place: returns `Editing` / `Commit` / `Cancel`; pads by `INLINE_PAD`, which
matches `style::text_highlight`), `alert(ui, icon, message, &buttons)` (a
BAlert; open its window with `WindowOptions::alert(title, message)`; the
last button is the default, Escape picks the first).

Colour emoji (`tabula_kit::emoji`, on `crates/emoji`): egui's text is
monochrome, so code that places glyphs itself calls
`emoji::paint(painter, rect, cluster)`, which draws the cluster in colour
fitted and centred in `rect` (a cached texture at device resolution) and
returns false when there is no colour glyph (draw text instead).
`emoji::wants_color(cluster, wide)` says whether a cluster has emoji
presentation; `emoji::continues(prev, next)` joins sequences a cell grid
split (ZWJ, skin tones, flags). `tabula-emoji` shapes with harfrust and
paints COLRv0/v1 with tiny-skia (Noto Color Emoji is COLRv1); the font is
read on a worker thread when the system fonts load. Render test images
with `EMOJI_OUT=/tmp/dir cargo test -p tabula-emoji`.

Pictures (`tabula_kit::images::open(path)`, re-exporting `crates/images`
so app_server can use it without the kit): everything the `image` crate
reads plus JPEG XL (jxl-oxide), AVIF (avif-parse and rav1d built without
asm, with our own YUV to RGB and alpha) and HEIC (heifer), EXIF-rotated,
with a 1 GiB allocation limit; returns the image and its format's name.
Keep its dependencies permissive: zenavif / rav1d-safe were dropped for
being AGPL. `TABULA_SAMPLES=dir cargo test -p tabula-images -- --ignored`
decodes example.avif and example.heic from `dir`. Thumbnails and Quick Look use
it. File panels (`tabula_kit::file_panel`): `FilePanel::open(dir)` /
`FilePanel::save(dir, name)` give a BFilePanel (path popup, Home, sortable
Name/Size/Modified list, "Save as:" field, Replace confirmation);
`FilePanel::folder(dir)` picks a folder (folders only; Select takes the
selected one, or the one shown). Open it
in its own window of `PANEL_SIZE`, call `panel.ui(ui)` each frame and act
on the `PanelResult` (`Chosen(path)` / `Cancelled`). Toolbars are
`metrics::TOOLBAR_HEIGHT` tall. `style::find_font(families, weight)`
loads an installed font as `(bytes, face index)` (call it off the UI
thread); `style::add_document_font(family, bytes, index)` makes it
`FontFamily::Name(family)` in every window of the app, including ones
opened later and after a reconnect, with the UI fonts as fallback. Check
`style::family_ready(ctx, family)` before naming it in a frame: egui
panics on unknown families, and new fonts arrive with the next frame. App settings live in
`tabula_kit::config_dir()` (`$XDG_CONFIG_HOME/tabula/NAME.json`); user
folders come from `tabula_kit::user_dir("PICTURES")` (user-dirs.dirs,
None if unset or missing).

Menus: `Menu::new("File").item(label, command, Some(Key::N))`,
`.item_shift(…)`, `.separator()`, `.submenu(Menu)`, `.checked(bool)`,
`.enabled(bool)`. Popups are real `xdg_popup`s drawn by the kit. Menus
don't scroll, so split long lists into submenus (StyledEdit's font
families). ⌘H/⌘K/⌘U/⌘W never reach a focused text field as egui's emacs
deletions: the kit holds them back from `ui` and hands them to the menus
(windows with `raw_keys` get them as usual). While a menu is open the
window hasn't the keyboard and egui collapses a text field's selection;
keep your own copy if a menu command acts on it (StyledEdit's `blurred`).
app_server hands the keyboard back to the window as soon as its last
popup is destroyed.

Alerts: `widgets::alert(ui, icon, message, buttons)` (or
`widgets::alert_frame(ui, icon)` for an alert with fields, which draws the
stripe and icon and returns the content column) in a window opened
with `WindowOptions::alert(title, message)` (sized for the message);
icons are BeOS's `warning`, `stop`, `info` and `idea`. The kit makes the
alert a child (`xdg_toplevel.set_parent`) of the app's focused window, or
of `WindowOptions::parent(id)` when given, and app_server opens any
window with a parent over it as BAlert does: centred across, a quarter
of the way down.

### Look and feel rules

1. **Colours only from the palette** (`style::palette(ctx)` or
   `cx.palette()`): `panel_background`, `document_background`,
   `control_*`, `menu_*`, `list_*`, `shine`, `keyboard_navigation`, … Use
   `style::shade(&p, rgba, tint)` for bevel shades (it compensates in dark
   mode). Never hard-code a grey: it will be wrong in the other theme.
   Semantic accent colours (graph lines, file-type tints) must be checked in
   both themes.
2. **Spacing only from `tabula_kit::metrics`** (`SPACING` 8, `TIGHT` 4,
   `WINDOW_INSET` 12, `BOX_INSET` 10, `LABEL_GAP` 8, `CONTROL_HEIGHT` 24,
   `LIST_ROW` 20, …). If you need a new kind of gap, add a named constant
   there rather than a literal in your app.
3. **Text**: draw single lines with `style::label(painter, x, center_y,
   align, text, font, color)`; it centres capitals optically and snaps to
   device pixels. Fonts: `style::plain_font()`, `style::bold_font()`,
   `style::italic_font()`, `style::mono_font(size)`. Highlight selected
   text with `style::text_highlight(galley, origin, size, pad)` (cap height
   to baseline, so every label's highlight is the same height); drag-select
   bands with `style::selection_band`.
4. **Lines are one logical pixel** (`style::hline`, `vline`, `frame_rect`,
   `bevel`, `sunken`, `raised`). Snap rects to the pixel grid
   (`style::snap`). No blurry half-pixel lines.
5. **Icons**: `tabula_kit::icons::paint(painter, rect, "name")` with names
   from `data/icons/manifest.txt` (or freedesktop names / absolute paths).
   Sizes 16 (lists, Deskbar), 32 (Tracker list), 48–64 (Tracker icon view),
   128 (About boxes). Image previews: `icons::thumbnail(ctx, path, size)`
   decodes off-thread and returns `None` until ready (it repaints when
   done); place it with `icons::fit(ctx, &tex, bounds)` so it is never
   upscaled or blurred. Need a new one? Follow **[ICONS.md](ICONS.md)**: the
   style guide, subject template and required generation workflow
   (sunburst at max quality, against the reference set, candidates
   reviewed on a contact sheet).
6. BeOS idioms to keep: yellow tab decorator (app_server's job), menu bar in
   every document window, ⌘ shortcuts (Ctrl on the wire: app_server's
   `keymap` turns the Mac ⌘ key into Ctrl for clients, Caps Lock too, and
   keeps ⌘Tab / ⌘\` (Twitcher), ⌘Space / tap ⌘ (Find), ⌘ arrows and ⌘⌫ for
   itself; terminals listed in `KeyboardSettings::terminal_apps` get ⌘ as
   Ctrl+Shift so physical Ctrl reaches the tty), keyboard focus shown as a navigation-blue underline/frame,
   Tracker-style spatial behaviour. Modernise freely where it helps
   (smooth scrolling, hi-dpi, search fields, sensible defaults).

## The session and in-place reload

Tabula upgrades without logging out. `tabula-session` (what the login
screen runs) owns the Wayland socket (`wayland-N`) and the IPC socket and
hands both to app_server, so clients never see the sockets disappear.

* `tabula-session reload` (SIGHUP): if `tabula-session` itself was
  upgraded it re-executes the new binary first, keeping its children and
  sockets. Then it stops app_server (SIGTERM; it saves every window's
  place to `$XDG_RUNTIME_DIR/tabula-layout-<display>.json`) and starts the
  one now on disk, which puts returning windows back (same app id and
  title within 60 s; position, workspace, minimized, focus).
* Kit apps notice the lost connection and reconnect (for up to 20 s). An
  app whose binary didn't change keeps everything and recreates its
  windows exactly. One whose binary changed calls `App::save_state`,
  re-executes itself (same pid and arguments), and reads the result with
  `cx.restored_state()` at start-up. **Every app with windows the user
  arranged must implement both**: save which windows are open (with
  `cx.window_size`), their documents/folders, view settings and
  selection, and reopen them in the same order. Tracker's `Saved` is the
  example.
* app_server or a shell component that crashes is restarted (up to 5
  times a minute). `tabula-session quit` ends the session.
* Never cache anything about the connection you can't rebuild; never
  replace an installed binary by writing over it (install to a temporary
  name and rename, as `tools/install-session.sh` does).

Install for the login screen with `sudo tools/install-session.sh`
(release build into `/usr/local`, plus
`/usr/local/share/wayland-sessions/tabula.desktop`). After rebuilding,
`tabula-session reload` (or Preferences → Restart Tabula) upgrades the
running desktop.

## Testing: headless sessions

Never open windows on the user's screen. `tools/dev/tsession` runs a
headless app_server (GPU, offscreen, 2560×1600 device pixels at 2×, i.e.
1280×800 logical) and drives it:

```sh
T=tools/dev/tsession
$T start mine --light          # unique session name per developer/agent
$T start mine --session [--shell]   # under tabula-session (with Deskbar + desktop)
$T start mine --session --home /tmp/tabula-dev/mine-home   # sandbox HOME
$T reload mine                 # in-place reload (test upgrades: rebuild, then reload)
$T run mine tabula-myapp       # binaries from target/debug (log in /tmp/tabula-dev/mine/)
$T start two 2560x1600,1920x1080   # two screens, HEADLESS-1 and -2 (each offers a few modes)
$T shot mine /tmp/x.png        # full screenshot (device pixels) of the pointer's screen
$T shot two /tmp/y.png --output HEADLESS-2
$T shot mine /tmp/p.png X Y W H --pointer   # with the pointer drawn in
$T zoom mine /tmp/z.png X Y W H 4   # crop in logical px, magnified 4× (nearest)
$T click mine X Y [right]      # logical coordinates (screenshot px / 2)
$T drag mine X1 Y1 X2 Y2
$T type mine "hello"           # $T key mine 29 30  → Ctrl+A (evdev codes)
$T theme mine dark
$T stop mine
```

sudo resets HOME, so without `--home` a session and its apps use the real
home folder: pass `--home` whenever a test saves files (Screenshot,
Tracker, StyledEdit…). Sessions are always silent (`PULSE_SERVER` points at
a dead socket), so tests never reach the real speakers or volume.

Clicks are global logical coordinates: on a screen that isn't at (0, 0),
add its position (`$T ipc NAME '{"type":"outputs"}'`). `start` rewrites
the session's app_server.json, so test persistence with `--session` and
`reload`.

Look at your screenshots. Then zoom in to 4× and check edges, insets,
alignment and text placement pixel by pixel, in **both** light and dark.
Compare against BeOS R5 / Haiku screenshots from memory. Fix anything that
looks even slightly off before calling something done.

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

`docs/journal/` is Tabula's visual history (images in Git LFS). Whenever
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
preference app per area. `tabula-preferences <panel>` opens or brings
forward a panel (Deskbar's Preferences menu does this); with no argument a
launcher lists them. The panel list Deskbar shows is
`tabula_xdg::apps::PREFERENCE_PANELS`; a unit test keeps it in step with
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
* Network (`network.rs`) reads NetworkManager through
  `tabula_system::network` on a worker thread (devices, addresses,
  networks in range merged by SSID, saved connections; a scan every 30 s)
  and joins (saved connection, or a new WPA-PSK one), disconnects and
  forgets. **Tests only look: never toggle Wi-Fi, Join, Disconnect or
  Forget on the real machine.**
* Bluetooth (`bluetooth.rs`) reads BlueZ (`tabula_system::bluetooth`,
  ObjectManager) every second, searches for 30 s when it opens or on
  Search, lists named or paired devices, and pairs, connects, disconnects
  and removes on a thread per action. Its pairing agent is registered for
  this process only (never `RequestDefaultAgent`) and shows passkeys and
  PIN requests in the panel. **Tests only look and search: never Pair,
  Connect, Remove or toggle Bluetooth on the real machine.**
* Touch ID (`touchid.rs`) lists, enrols (`fingerprint::enroll`, with a
  StatusBar per touch, cancellable) and removes fingers through fprintd.
  fprintd only answers a process in an active local session; headless, the
  panel says so. `TABULA_FAKE_FINGERS=right-index-finger,…` (debug builds)
  fakes a sensor so the whole panel can be exercised headless. **Never
  enrol or remove on the real sensor in tests.**
* Date & Time (`datetime.rs`) talks to systemd-timedated through
  `tabula_system::time` on its own worker thread (status every 2 s; set
  time, zone and network time with polkit's interactive auth, so a
  password prompt never holds the window). Zone offsets and times come
  from jiff and the system tz database. Deskbar's clock format is
  `Settings::clock` (`ClockSettings::format` is shared by Deskbar and the
  panel's preview). **Tests must never press Set, Set Time Zone or the
  network-time box: they change the real machine.**
* Backgrounds (`workspaces.rs`): a colour per workspace and one wallpaper.
  Choose… opens a FilePanel (picture types only) as a child window; the
  text field follows the setting when it changes elsewhere (Choose…,
  Defaults, Revert). app_server draws the wallpaper (`wallpaper.rs`):
  each output's picture is decoded (JPEG XL too, via jxl-oxide), scaled
  to cover and cropped on a worker thread, kept in the output's user
  data and drawn under every surface; the workspace colour shows until
  it is ready or if the file can't be read.
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
  no output (Deskbar, the desktop): they carry `shell::AutoOutput` and
  move when the main display changes or their output is unplugged.
  Identify (`IdentifyOutputs`) shows each display's name for 4 s
  (app_server `identify.rs`). Settings are kept per monitor under
  `tabula_protocol::display_key` ("make model serial" from the EDID,
  else the connector; `OutputInfo::key`), so they follow a monitor to
  another port; always go through `DisplaySettings::config` /
  `config_mut`, which also read and migrate older connector entries.
  Mirroring: `OutputConfig::mirror` names the source's key (`Some("")`
  means extend). `desktop::apply_mirroring` takes a mirror out of the
  Space into `State::mirrors` (so windows, the pointer and layers never
  land on it; `OutputInfo::mirror_of` reports it) and the backends draw
  the source's elements on it through `render::FrameElements::Mirror`,
  scaled to fit and centred (`render::fit`). Preferences hides a mirror
  from the arrangement and offers only mode, refresh and "Use as".
* Memory buffers drawn by app_server (twitcher, wallpaper, identify) must
  pass the full buffer as `src` to `MemoryRenderBufferRenderElement::
  from_buffer`; without it the buffer counts as logical pixels and is
  drawn at twice its size on a 2× screen.
* About (`about.rs`) has Restart Tabula: SIGHUP to `$TABULA_SESSION_PID`,
  the same in-place reload as `tabula-session reload` (disabled outside
  a session).

## Power (crates/app_server/src/power.rs)

app_server owns idleness: any input (or a synthetic event) is activity; a
5 s tick turns the displays off (`settings.power.display_off_minutes`) and
then asks logind to suspend (`sleep_minutes`). Caffeinated mode
(`Request::SetCaffeinated`, the Deskbar cup) and apps holding a
zwp_idle_inhibit inhibitor count as activity. `Request::PowerStatus` and
`Event::PowerChanged` report it all, with the built-in display's brightness
(`Request::SetBrightness`, perceptual 0–1). Brightness and volume keys are
taken by app_server.

Hardware is touched only when app_server is the session
(`Backend::IS_SESSION`), from a worker thread: the backlight through
logind `SetBrightness` (no root needed), the inhibitor, suspend, the sound
server. Headless sessions simulate a backlight at 0.75 and only log
"would sleep", so tests can drive all of it safely.

## The screen saver (crates/screensavers, apps/screensaver, app_server saver.rs)

The modules are R5's own list, in its order (Blackness, BuyNow, Flip,
Icons, Lens, Lissart, Message, Spiral, Spots, StringTheme, SuperString;
from period screenshots of R5's ScreenSaver preferences). Each is a
`tabula_screensavers::Module` that draws a whole frame into a rect with an
egui painter, so the same code runs full screen and in the preview monitor
of Preferences ▸ Screen Saver. Size things with `Frame::k()`, step motion
with the fixed 60 Hz `Clock`, and add settings as `Opt`s (stored as strings
in `settings.screen_saver.options[module]`; Preferences builds their
controls). Modules with `uses_desktop` (Flip, Lens) get a picture of the
desktop that app_server takes as the saver starts.

app_server starts `tabula-screensaver` after `screen_saver.run_minutes`
idle, from `Request::StartScreenSaver` (Test) or when the pointer enters
the "Fade now" corner, on a private connection (`spawn_private`, like the
lock screen); the "Don't fade" corner and caffeinated mode hold it off. Any
input disconnects it (after 1.5 s grace for a deliberate start). With
`screen_saver.lock` it locks underneath after `lock_minutes`, and keeps
drawing over the lock screen (`Lock::over`) until the first input. The
displays turning off ends it.

Test headless: `tsession ipc NAME '{"type":"start_screen_saver"}'`, or set
`screen_saver.module` with get/set_settings first.

## The pointer (crates/hvif, app_server cursor.rs)

The pointer is Haiku's system cursor set (`crates/hvif/cursors`, from
Haiku's CursorData.cpp, MIT; see the LICENSE there). `tabula-hvif` reads
the Haiku Vector Icon Format and draws it as Haiku's IconRenderer does
(shapes in order, 2.2-gamma blending, level-of-detail, stroke and contour
transformers); `tabula_hvif::cursor::render(shape, scale)` draws a cursor
as Haiku's CursorManager does: 22 logical pixels, the icon's top-left
32 × 32 region, a soft shadow, the hotspot scaled. app_server's
`cursor.rs` maps CSS cursor names (from its own decorations, or clients'
cursor-shape-v1 requests) to them, caches each shape per scale and
hands the backend a buffer whose pixels land one-to-one on the screen.
Clients that set a cursor surface still get theirs. To see every cursor:
`TABULA_CURSOR_SHEET=/tmp/c.png cargo test -p tabula-hvif sheet`.

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
Tabula ▸ Stay Awake, for durations and the lid); Ctrl+⌘A toggles it from
the keyboard. The keyboard backlight (`tabula_system::backlight::keyboard`, an LED
class device set through logind) follows its keys and Preferences ▸
Keyboard, and goes off with the displays. The brightness, keyboard and
volume keys and Ctrl+⌘A show `osd.rs`'s
panel low on the screen under the pointer for 1.6 s (in a real session
the worker reports the volume it set; headless simulates a volume).
`TABULA_OSD_SHEET=/tmp/o.png cargo test -p app_server osd_sheet` draws
every state.

## Night light (app_server night.rs)

Preferences ▸ Appearance's night light (off, on, or scheduled, at a
colour temperature) is resolved against the clock in
`desktop.rs::update_appearance`; `night.rs` fades the screens' white point
there over 1.5 s. The udev backend loads each CRTC's gamma ramps scaled
by the gains (`Backend::set_white_point`), so it costs no rendering and
screenshots stay neutral; new screens and a returning VT session get the
current gains. At start-up the gains are set at once, so a reload keeps
the warmth without a flash. Headless logs each step ("white point") for
tests.

## Screenshots (app_server screenshot.rs, apps/screenshot)

`Request::Screenshot { path, output, window, border, pointer }` (only
`path` is required) saves a PNG of one output (the one the pointer is on
by default) or of one window, and replies once the file is complete. It
becomes a `screenshot::Shot` for one output, which every backend takes
with that output's next frame (`screenshot::take`): the screen via
`render::capture`, or the window alone over transparency via
`render::offscreen` (its tab and border with `border`, keeping the tab's
shape, or only the client's area). A thread then draws the pointer in
(the cursor it is showing, from `cursor.rs`), unpremultiplies, encodes and
writes `NAME.part` and renames it, so readers never see half a file.
Window shots are refused while the screen is locked.

The Screenshot app (`tabula-screenshot`) follows Haiku's: it takes a
picture as it starts, then shows it with Capture active window / Include
window border / Include mouse pointer / Delay, and Name / Save as (PNG,
JPEG, BMP, TIFF, WebP) / Save in (Home, Desktop, Pictures, Choose
folder…). New screenshot minimizes its own window first (so it isn't in
the picture and the active window is the one behind it) and brings it
back; Save writes `screenshotN` and quits; Copy to clipboard offers
`image/png`. Settings are in `screenshot.json`. `--silent [--window]` saves
straight into the folder with no window: app_server runs it for ⌘⇧3 and
Print Screen (the screen) and Alt+Print Screen (the active window).
`--region` (⌘⇧4, region.rs) captures every output, covers each with an
exclusive overlay showing its picture dimmed, and saves the rectangle you
drag (its size in pixels follows the corner); Escape or a click cancels.
⌘⇧5 opens the app.

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
JSON) go in the `user.tabula.styles` attribute, alignment and wrapping in
`user.tabula.alignment` / `user.tabula.wrap`, so other programs see plain
text. Styles that no longer fit the text (edited elsewhere) are dropped.
Non-UTF-8 files open as Latin-1 and save as UTF-8 (the status line says
so); unwritable files are read-only (selectable, not editable). Saving
writes `.NAME.saving` beside the file, copies its permissions and renames
it over the original, on a thread.

`doc.rs` is the model and is unit-tested. The `TextEdit` edits a plain
`String`; its layouter calls `Doc::sync`, which diffs the new text against
the old (common prefix and suffix on char boundaries) and moves the runs:
inserted text takes the pending typing style, the style of what it
replaced, or the style before it, and pasting what was copied here brings
its styles back. Undo is our own (snapshots, typing and deleting coalesced
within 2 s); ⌘Z/⌘⇧Z/⌘Y are consumed before the field sees them. Replace
All splices each match so the styles between them survive. egui paints
selected text in one colour, so the selected glyphs are drawn again over
the highlight in their own colours (`selected_glyphs`).

`fonts.rs` owns a font database on a worker thread: it lists families for
the Font menu (in submenus titled like a dictionary's guide words) and
loads a face the first time a style names it, registering it as
`se:Family:R|B|I|BI` with `style::add_document_font`; until then the UI
font stands in. Find and Replace are Haiku's windows (case, wrap-around,
backwards, replace all), shared by every document; ⌘G finds again, ⌘H
finds the selection, ⌘T replaces the same. `save_state` keeps every
window, with unsaved text and styles, through a reload.

## Activity Monitor (apps/activity)

ProcessController and Pulse in one window: Graphs and Processes tabs.
`sample.rs` reads everything from /proc and /sys on a worker thread
(`probe` finds the sources once: CPU, memory, cache, swap, network and
disk rates, faults, context switches, processes and threads, frequency,
battery, every core, and every hwmon temperature, power and fan with a
readable name); the interval and whether to read the process table are
atomics the window sets (`Control`), so the table is only read while its
tab shows. Rates are per second whatever the interval.

Each graph is a list of source keys (`activity.json`), scaled by the
source's own maximum or a nice ceiling of what's shown; the context menu
toggles sources (Processors and Sensors in submenus) and adds or removes
graphs. The process list sorts by any column and filters by name or
exact ID and by owner; the selection follows the PID and clears when the
process is hidden or exits, so a re-sort never moves it onto another
process. Quit sends SIGTERM; Force Quit asks first, then SIGKILL; EPERM
is an alert. **In tests, only ever quit a process you started, selected
by its exact ID.**

## The polkit agent (apps/polkit-agent, tabula_system::polkit)

Asks for a password when a program needs an administrator (timedated,
logind, NetworkManager, …). `tabula-session` starts it only in a real
`--tty-udev` session, with `TABULA_REAL_SESSION=1` (cleared for nested
and headless sessions); without that it refuses to run, because it
registers with polkitd for the whole **login session** and must never take
over the prompts of the session it was started from. It stays resident
(`Cx::stay_resident`) and opens a BAlert-style window per request
(`widgets::alert_frame`). Proving identity runs polkit's setuid
`polkit-agent-helper-1` (the only thing polkitd accepts an answer from):
the cookie goes on its stdin, and its PAM conversation (`PAM_TEXT_INFO`
from fprintd for Touch ID, then `PAM_PROMPT_ECHO_OFF` for the password)
is relayed to the window; a failure starts it again for another try.

Test it with `tsession run NAME tabula-polkit-agent --self-test ACTION`:
it serves only its own process and asks polkitd about ACTION with
`CheckAuthorization` (which changes nothing), printing the outcome to
its log. **Cancel; never type the user's password, and only check
actions, never perform them.** `TABULA_POLKIT_HELPER` points at a fake
helper for unit tests.

## The lock screen (crates/app_server/src/lock.rs, apps/lock)

`lock_screen()` (Ctrl+⌘Q, Deskbar ▸ Lock Screen, `Request::Lock`, the
display turning off for idleness when `power.lock_on_display_off`, logind
`PrepareForSleep` and `Lock`) covers every output in black at once, takes
keyboard focus away and starts `tabula-lock` on a socket pair of its own
(WAYLAND_SOCKET): ext-session-lock is offered to that one client only. The
locker is restarted if it dies (at most every 2 s); the screen stays
black meanwhile, so the lock never fails open. While locked, input reaches
lock surfaces only, keybindings are off, and the displays sleep after a
minute. A marker, `$XDG_RUNTIME_DIR/tabula-locked-$WAYLAND_DISPLAY`
holding tabula-session's pid, makes an app_server that reloaded or crashed
come back locked (only in the same session: display names get reused). Before sleep,
app_server holds a logind delay inhibitor until every output shows the lock
surface (or 3 s).

`tabula-lock` uses the kit's `cx.lock_session()` (`SessionLock::Locked`
gives one window per output, the first is primary; `cx.unlock_session()`
ends it) and checks the password with `tabula_system::auth` (PAM via
libpam, on a thread) while listening for Touch ID with
`tabula_system::fingerprint`. Debug builds take `TABULA_PAM_CONFDIR` +
`TABULA_PAM_SERVICE` (passed through by tsession), so tests use a private
pam_exec service and never a real password:

```sh
printf 'auth required pam_exec.so expose_authtok quiet /path/check.sh\naccount required pam_permit.so\n' > $dir/exec
TABULA_PAM_CONFDIR=$dir TABULA_PAM_SERVICE=exec tools/dev/tsession start lk --light
tools/dev/tsession ipc lk '{"type":"lock"}'; tools/dev/tsession type lk secret; tools/dev/tsession key lk 28
```
