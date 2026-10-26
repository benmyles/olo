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
| `crates/preview` | `tabula-preview`: native previews of every file kind (text, images, PDF, audio, video, archives, office documents, fonts). Used by Tracker's Quick Look and the Viewer. |
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
  off-thread and delivers an `egui::Event::Paste`), `quit`, `palette`,
  `proxy`, `window_size`, `restored_state`. There is no clipboard manager,
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
  them; the screen saver opens one per output).
* `ui.ctx().set_cursor_icon(CursorIcon::None)` hides the pointer over a
  window (screen savers).
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
multi-select, context clicks), `TabView`, `color_well`, `heading`,
`form_label`, `window_content`, `button_row`, `context_menu`,
`popup_menu_at` / `popup_result`, `InlineEdit` (Tracker-style rename in
place: returns `Editing` / `Commit` / `Cancel`; pads by `INLINE_PAD`, which
matches `style::text_highlight`), `alert(ui, icon, message, &buttons)` (a
BAlert; size the window with `alert_height(message)` × `ALERT_WIDTH`; the
last button is the default, Escape picks the first).

File panels (`tabula_kit::file_panel`): `FilePanel::open(dir)` /
`FilePanel::save(dir, name)` give a BFilePanel (path popup, Home, sortable
Name/Size/Modified list, "Save as:" field, Replace confirmation);
`FilePanel::folder(dir)` picks a folder (folders only; Select takes the
selected one, or the one shown). Open it
in its own window of `PANEL_SIZE`, call `panel.ui(ui)` each frame and act
on the `PanelResult` (`Chosen(path)` / `Cancelled`). Toolbars are
`metrics::TOOLBAR_HEIGHT` tall. `style::find_font(families, weight)`
loads an installed font as `(bytes, face index)` (for registering extra
egui families; call it off the UI thread). App settings live in
`tabula_kit::config_dir()` (`$XDG_CONFIG_HOME/tabula/NAME.json`); user
folders come from `tabula_kit::user_dir("PICTURES")` (user-dirs.dirs,
None if unset or missing).

Menus: `Menu::new("File").item(label, command, Some(Key::N))`,
`.item_shift(…)`, `.separator()`, `.submenu(Menu)`, `.checked(bool)`,
`.enabled(bool)`. Popups are real `xdg_popup`s drawn by the kit.

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
$T shot mine /tmp/x.png        # full screenshot (device pixels)
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

Look at your screenshots. Then zoom in to 4× and check edges, insets,
alignment and text placement pixel by pixel, in **both** light and dark.
Compare against BeOS R5 / Haiku screenshots from memory. Fix anything that
looks even slightly off before calling something done.

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

## Screenshots (app_server screenshot.rs, apps/screenshot)

`Request::Screenshot { path, output, window, border, pointer }` (only
`path` is required) saves a PNG of one output (the one the pointer is on
by default) or of one window, and replies once the file is complete. It
becomes a `screenshot::Shot` for one output, which every backend takes
with that output's next frame (`screenshot::take`): the screen via
`render::capture`, or the window alone over transparency via
`render::offscreen` (its tab and border with `border`, keeping the tab's
shape, or only the client's area). A thread then draws the pointer in
(the default xcursor, from `cursor.rs`), unpremultiplies, encodes and
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
Print Screen (the screen) and ⌘⇧4 and Alt+Print Screen (the active
window); ⌘⇧5 opens the app.

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
