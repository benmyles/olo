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

## Layout

| Path | What |
|---|---|
| `crates/app_server` | The compositor (Smithay, forked from Anvil). Draws the BeOS window decorator (yellow tabs), manages windows/workspaces, serves IPC. |
| `crates/session` | `tabula-session`: starts and supervises app_server, Deskbar and the desktop; in-place reload and upgrade (see below). |
| `crates/protocol` | JSON-lines IPC between app_server and our apps (`Client`, `Request`, `Reply`, `Event`, `Settings`). |
| `crates/theme` | `Palette` (light/dark), BeOS tint helpers, decorator metrics, font lists. |
| `crates/kit` | The Interface Kit: Wayland client platform, GPU rendering (egui on wgpu/Vulkan), menus, BeOS widgets, icons, `.desktop` entries. **All apps are built on it.** |
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
  `copy` (clipboard), `quit`, `palette`, `proxy`, `window_size`,
  `restored_state`.
* Background work goes on threads; post results with `Proxy::send` and
  handle them in `App::message` (then `cx.redraw(window)`).
* Periodic refresh: `ui.ctx().request_repaint_after(duration)`.
* Layer-shell surfaces (Deskbar, desktop): `WindowKind::Layer(LayerOptions{…})`.

### Widgets (`tabula_kit::widgets`)

`Button` (`.default(true)` for the Enter button), `CheckBox`, `RadioButton`,
`TextControl` (`.label("Name:").divider(x)`), `TextView`, `Slider`
(`.label().value_text().limits()`), `MenuField` (pop-up choice), `BBox`
(etched group box with label), `separator`, `StatusBar` (progress),
`ScrollView` (`show`, `show_rows` for virtualised content, `reveal`,
`.auto_hide(true)` to drop the bar when content fits),
`ColumnListView` + `Column` + `ListState` (sortable, resizable,
multi-select, context clicks), `TabView`, `color_well`, `heading`,
`form_label`, `window_content`, `button_row`, `context_menu`,
`popup_menu_at` / `popup_result`, `InlineEdit` (Tracker-style rename in
place: returns `Editing` / `Commit` / `Cancel`; pads by `INLINE_PAD`, which
matches `style::text_highlight`), `alert(ui, icon, message, &buttons)` (a
BAlert; size the window with `alert_height(message)` × `ALERT_WIDTH`; the
last button is the default, Escape picks the first).

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
   every document window, ⌘ shortcuts (Ctrl on the wire; keyd maps the Mac
   ⌘ key), keyboard focus shown as a navigation-blue underline/frame,
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
