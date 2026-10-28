# Hacking on Olo

Olo is a Wayland desktop environment in Rust that captures the look and
feel of BeOS R5, with deliberate modern polish. **North star: everything is
pixel-perfect and beautiful, in light, dark and night modes, at 2× (and any
other) scale.** Every deviation from BeOS should be an intentional
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
   the desktop.
7. **Stay current without polling.** What is on screen reflects the system
   as it changes: watch files with inotify (Tracker's `watch.rs`: one thread,
   changed items stat'ed off the UI thread, bursts gathered for ~30 ms,
   a full relist on queue overflow) and the mount table with `POLLPRI` on
   `/proc/self/mountinfo`. No timers that re-read the disk to look for
   changes.
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

## Layout

| Path | What |
|---|---|
| `crates/app_server` | The compositor (Smithay, forked from Anvil). Draws the BeOS window decorator (yellow tabs), manages windows/workspaces, serves IPC. |
| `crates/session` | `olo-session`: starts and supervises app_server, Deskbar and the desktop; in-place reload and upgrade (see below). |
| `crates/protocol` | JSON-lines IPC between app_server and our apps (`Client`, `Request`, `Reply`, `Event`, `Settings`). |
| `crates/theme` | `Palette` (light/dark), BeOS tint helpers, decorator metrics, font lists. |
| `crates/kit` | The Interface Kit: Wayland client platform, GPU rendering (egui on wgpu/Vulkan), menus, BeOS widgets, icons, `.desktop` entries. **All apps are built on it.** |
| `crates/xdg` | `olo-xdg`: freedesktop plumbing shared by app_server and the kit (re-exported there): data dirs, `.desktop` entries, icon lookup/decoding (`load_icon`), our own apps (`apps::our_app`), MIME types (`mime::for_path`), opening files and URLs with the right app (`open::open`, `open::handler_cached`: never `xdg-open`; Olo's apps first, except that links and web pages go to the browser chosen in mimeapps.list, else `olo-chrome`), `find_program`, `spawn_detached`. No egui. |
| `crates/system` | `olo-system`: the system over its own protocols, in process: logind (`power`: reboot, power off, suspend, inhibitor locks), NetworkManager (`network`: status, join including 802.1X, forget), the sound server (`audio`: volume, mute, `watch` change feed), dconf writes, the D-Bus/systemd activation environment. All blocking: call from worker threads. |
| `crates/pool` | `olo-token-pool`: token pooling for Claude Code and Codex (see "Token Pool"): the accounts, their sealed store, the drain order and the loopback gateway. |
| `crates/ai` | `olo-ai`: how AI features reach their models (see "AI features"): `ask(&settings.ai, &feature, &Ask)` → the answer's JSON. Blocking; call off the UI thread. |
| `crates/preview` | `olo-preview`: native previews of every file kind (text, images, PDF, audio, video, archives, office documents, fonts). Used by Tracker's Quick Look (`Preview::new`, images capped at 3200 px) and the Viewer (`Preview::new_zoomable`: full resolution up to the GPU texture limit, zoom and pan, `zoom` / `set_image_scale`). Images carry halved copies (`load::halvings`) because egui-wgpu makes no mipmaps. Video (`video.rs`, H.264 in mp4 or Matroska and AV1 in Matroska/WebM, with `media.playable` set): a worker demuxes and decodes (OpenH264, or rav1d through `olo_images::av1::Decoder`) up to six frames ahead, giving each picture the earliest outstanding presentation time (OpenH264 returns display order; it must be made with `Flush::NoFlush`, as the crate's default flush after every packet breaks B-frame reordering and every B-frame then fails), mp4 times shifted by the track's edit list; `Video::tick` shows frames against a clock that follows the sound's reported position when an `audio::Player` is playing it and runs alone otherwise (no sound server in tests, silent files). Seeks go to the key frame before the target (Matroska: scans back for one, further each time) and drop pictures before it; a paused seek still shows its frame. `OLO_SAMPLES=dir cargo test --release -p olo-preview video -- --ignored` checks seeking on test.mp4/mkv (one key frame) and gop.mp4/mkv (one a second), made with ffmpeg's testsrc2, plus av1.webm (svt-av1, Opus sound) and bframes.mp4 (any x264-made H.264 High clip with B-frames; `b_frames` checks every picture comes out, in order); `keeps_up` needs av1-1080.mkv (1080p AV1) to decode at well over 30 frames a second (72 on the M2 Max; the YUV to RGB fast path is fixed-point with tables, checked against the float path by `yuv_fast_path`). Audio decodes with symphonia plus `opus.rs` (opus-decoder, OpusHead parsing and pre-skip trim) for Ogg Opus. Picture editing (`edit.rs`): an `Editor` keeps a list of `Op`s (rotate, flip, crop, resize) with undo/redo and a saved mark, and a worker that loads the full picture once, applies new steps to its last result, sends `EditEvent::Shown` (downscaled for the screen, with halvings) for `Preview::show_edited`, and writes PNG, JPEG (quality 92), WebP, BMP, TIFF or GIF atomically (`write`, through a hidden `.name.saving` file); `edit::editable` says which files it can write back. `CropTool` is the crop overlay (drag a frame, move it or its edges; `Preview::image_rect` gives where the picture is drawn, `set_still` stops pan and zoom while it is up). The Viewer's Image menu and File ▸ Save / Save As / Revert use it; Quick Look's footer has Rotate Left / Right (⌘L / ⌘R), which save at once. |
| `apps/<name>` | One crate per application, binary named `olo-<name>`. Picked up by the workspace glob. |
| `vendor/` | Patched copies of dependencies (`[patch.crates-io]`), each explained in `vendor/README.md`; drop them when upstream has the fix. |
| `data/icons` | Icon set, pre-scaled to device sizes (`data/icons/<px>/<name>.png`). |
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
* **`Cx`**: `open_window`, `close_window`, `set_title`, `resize`, `redraw`,
  `activate` (bring a window forward and focus it, restoring it if it is
  minimized or on another workspace — via xdg-activation; use it when a
  forwarded launch asks for a window that is already open), `copy` (text
  to the clipboard), `copy_secret(text, clear_after)` (marks the offer
  `x-kde-passwordManagerHint: secret` so clipboard histories skip it, and
  withdraws it after `clear_after` if it is still ours), `copy_data` (any
  MIME types, e.g. `image/png`; the data is written to pasters from a thread), `paste` (reads the clipboard
  off-thread and delivers an `egui::Event::Paste`), `quit`, `stay_resident` (keep running with no windows), `palette`,
  `proxy`, `window_size`, `restored_state`, `activation_token(window,
  tag)` (an xdg-activation token from the latest input in the window,
  delivered as an `ActivationToken { tag, token }` message, to hand to
  another program so its window may come forward), `set_keyboard(window,
  Keyboard)` (a layer window taking the keyboard or not). Worker threads may call
  `ctx.request_repaint()` (or `_after`) on a window's context: the kit
  wakes its loop through egui's repaint callback and redraws that window.
  There is no clipboard manager,
  so an app that quits (or closes its last window) while it holds the
  clipboard stays running, windowless, until something else is copied.
  Its windows are gone then: a later launch (forwarded to it) must open
  a new one, so check `cx.is_open(id)` before activating a window you
  kept.
* **Single instance** (`olo_kit::instance`): call `instance::forward(app_id,
  &args)` first in `main` (returns true if a running copy took the
  arguments; exit then), and `instance::listen(app_id, cx.proxy())` in the
  init closure; forwarded launches arrive in `App::message` as an
  `instance::Launch { args, cwd }`.
  `instance::running(app_id)` says whether a copy is listening. An app
  with several windows should open a new one for `--new-window` (always,
  rather than bringing one forward) and be listed in
  `apps::NEW_WINDOW`: Tracker then offers **New Window** on its program
  (or a link to it) while it runs, and Deskbar in its menu, forwarding `--new-window` to the
  running copy (`apps::app_id_of` maps `olo-x` to `olo.x`). Agents
  do the same with `olo-x --new-window`.
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
  (`commands::invoke(pid, command)`). Both block for at most a second;
  call them off the UI thread.
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
  egui gives its own widgets roles and names; text
  painted by hand is only pixels, so `style::label` and `Cell` describe
  theirs with `style::describe_text(ctx, role, text, rect, selected)`.
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
  holds such a release over to the next frame (`input::split_taps`), so
  egui sees the button down for a frame and press-and-hold controls
  (menus, scroll arrows, drags) work from taps too.
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

### Widgets (`olo_kit::widgets`)

`StringView` (a label; `.dim(true)` for hints, `.wrap(true)` for a paragraph, `.color(c)`), `Button` (`.default(true)` for the Enter button), `CheckBox`, `RadioButton`,
`Button::pushed(true)` for a toolbar toggle that stays down,
`TextControl` (`.label("Name:").divider(x)`), `TextView` (`.highlight(&f)`
colours it: `f(text, font, colour)` returns a `LayoutJob` in sections, a `widgets::Highlighter`,
as Log Viewer's syntax highlighting does), `Slider`
(`.label().value_text().limits()`), `MenuField` (pop-up choice), `BBox`
(etched group box with label), `separator`, `StatusBar` (progress; `level_bar(painter, rect, fraction,
fill)` paints just its bar, for list cells),
`ThumbnailPicker` (a wrapping grid of `Thumbnail{texture, color, label}`
to choose one from, ringed when chosen, arrow keys move; Backgrounds'
wallpapers), `CellGrid` (Pulse's LED meters: a heatmap of small lit cells in sunken
wells, `GridCell{color, mark, name, selected}` each; `.rows(n)
.cell_size(px)` fixes its height, and with more cells than fit they
shrink so more rows and columns fit the same space; `.slots(n)` shows
at least n wells, `.fill()` wells across the whole width; each cell is
clickable, named for agents, its name a tooltip, the selected one
ringed; `show(ui).clicked` is the cell clicked, by index),
`ScrollView` (`show`, `show_rows` for virtualised content, `show_in(ui,
rect, |ui, viewport|)` for content you lay out yourself, `reveal`,
`ScrollView::scroll_to(ctx, id, offset)`, `.auto_hide(true)` to drop the
bar when content fits; a drag held within 24 px of an edge scrolls it on; `SCROLL_BAR` is its width), `ScrollBar`
(`::vertical(id).show(ui, rect, &mut offset, content, view)`, a lone bar
for content the kit doesn't scroll, such as a web page), `ColorControl`
(BColorControl: grey/red/green/blue ramps and number fields for an
`&mut [u8; 3]`),
`ColumnListView` + `Column` + `ListState` (sortable, resizable,
multi-select, context clicks; `.reveal(row)` scrolls a row selected in code
into view; `.draggable()` reports `ListResponse::drag_started` and the
rows on screen, `visible` / `row_at(pos)`, for drop targets; in a
multi-select list, dragging from below the rows, or from a row's other
columns when it isn't selected, draws a selection band, as in Tracker's
list view: ⇧ or ⌘ adds to the selection, and it scrolls on near the
edges; on a draggable list the name column and selected rows drag the
items; `.empty_text("No lines match.")` says so in an empty list; a cell
draws plain text with `text`/`text_with` or sections of their own colours
with `text_job`), `TabView`, `TabStrip` (a row of document tabs under
a menu or tool bar, `STRIP_HEIGHT` (30) tall, roomier than a window tab: `StripTab { label, icon, color,
status }` each, the selected one open into the content, close boxes,
middle-click to close, a + button, drag along the row to reorder; it
returns a `StripAction` (`Select`, `Close`, `New`, `Menu(i, pos)` for a
right-click, `Move { from, to }`); each tab is a `Tab` in the
accessibility tree with its status in its name), `color_well`, `heading`,
`form_label`, `window_content`, `button_row`, `context_menu`,
`popup_menu_at` / `popup_result`, `InlineEdit` (Tracker-style rename in
place: returns `Editing` / `Commit` / `Cancel`; pads by `INLINE_PAD`, which
matches `style::text_highlight`), `alert(ui, icon, message, &buttons)` (a
BAlert; open its window with `WindowOptions::alert(title, message)`; the
last button is the default, Escape picks the first).

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
`olo_kit::config_dir()` (`$XDG_CONFIG_HOME/olo/NAME.json`); user
folders come from `olo_kit::user_dir("PICTURES")` (user-dirs.dirs,
None if unset or missing). Bytes too many to keep in memory (a download
being gathered) go in `olo_xdg::scratch_file()`: a nameless file under
`olo_xdg::cache_dir()`, on disk rather than /tmp, gone once closed.

Menus: `Menu::new("File").item(label, command, Some(Key::N))`,
`.item_shift(…)`, `.separator()`, `.submenu(Menu)`, `.checked(bool)`,
`.enabled(bool)`, `.icon(name)` (the last item's icon, any name
`icons::paint` takes; the generic application's if it can't be found).
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
popup is destroyed.

Alerts: `widgets::alert(ui, icon, message, buttons)` (or
`widgets::alert_frame(ui, icon)` for an alert with fields, which draws the
stripe and icon and returns the content column; `alert_text(ui, column,
top, text)` writes wrapped text in it and `alert_buttons(ui, column,
buttons, ready)` puts the buttons along its bottom, the default one
disabled until `ready`, with Enter and Escape) in a window opened
with `WindowOptions::alert(title, message)` (sized for the message);
icons are BeOS's `warning`, `stop`, `info` and `idea`. The kit makes the
alert a child (`xdg_toplevel.set_parent`) of the app's focused window, or
of `WindowOptions::parent(id)` (or `parent_opt(Option<id>)`) when given, and app_server opens any
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
2. **Spacing only from `olo_kit::metrics`** (`SPACING` 8, `TIGHT` 4,
   `WINDOW_INSET` 12, `BOX_INSET` 10, `LABEL_GAP` 8, `CONTROL_HEIGHT` 24,
   `LIST_ROW` 20, …). If you need a new kind of gap, add a named constant
   there rather than a literal in your app.
3. **Text**: draw single lines with `style::label(painter, x, center_y,
   align, text, font, color)`; it centres capitals optically and snaps to
   device pixels. Fonts: `style::plain_font()`, `style::bold_font()`,
   `style::italic_font()`, `style::mono_font(size)`. Highlight selected
   text with `style::text_highlight(galley, origin, size, pad)` (cap height
   to baseline, so every label's highlight is the same height); drag-select
   bands with `style::selection_band`. Keys as on a Mac menu ("⇧⌘←", each
   symbol on the text's baseline, though they come from a fallback font):
   `style::key_combo(painter, right_center, keys, color)` (and
   `style::shortcut` for a ⌘ menu shortcut).
4. **Lines are one logical pixel** (`style::hline`, `vline`, `frame_rect`,
   `bevel`, `sunken`, `raised`). Snap rects to the pixel grid
   (`style::snap`). No blurry half-pixel lines. An outline's fold mark
   is `style::latch(painter, rect, open, color)` (BOutlineListView's
   triangle: right when folded, down when open).
5. **Icons**: `olo_kit::icons::paint(painter, rect, "name")` with names
   from `data/icons/manifest.txt` (or freedesktop names / absolute paths).
   Sizes 16 (lists, Deskbar), 32 (Tracker list), 48–64 (Tracker icon view),
   128 (About boxes). Olo's wordmark is `olo_kit::brand::wordmark(painter,
   rect, dark)` (from `data/brand`, shrunk crisply to the height asked
   for; `wordmark_width(h)` to lay it out); Deskbar's tile shows it alone,
centred (beside the `olo` mark it read as "o olo"). Image previews: `icons::thumbnail(ctx, path, size)`
   decodes off-thread and returns `None` until ready (it repaints when
   done); place it with `icons::fit(ctx, &tex, bounds)` so it is never
   upscaled or blurred. Need a new one? Follow **[ICONS.md](ICONS.md)**: the
   style guide, subject template and required generation workflow
   (sunburst at max quality, against the reference set, candidates
   reviewed on a contact sheet).
6. BeOS idioms to keep: yellow tab decorator (app_server's job), menu bar in
   every document window, ⌘ shortcuts (Ctrl on the wire: app_server's
   `keymap` turns the Mac ⌘ key into Ctrl for clients, Caps Lock too, and
   keeps ⌘Tab / ⌘\` (Twitcher), ⌘ arrows and ⌘⌫ and the desktop shortcuts
   (see "Desktop shortcuts and tiling") for itself; terminals listed in
   `KeyboardSettings::terminal_apps` get ⌘ as Ctrl+Shift so physical Ctrl
   reaches the tty; `KeyboardSettings::num_lock` turns Num Lock on at start
   and when changed, and `apply_input_settings` keeps the Num Lock key's
   state across keymap changes, re-advertising modifiers to the focused
   client), keyboard focus shown as a navigation-blue underline/frame,
   Tracker-style spatial behaviour. Modernise freely where it helps
   (smooth scrolling, hi-dpi, search fields, sensible defaults).

## The session and in-place reload

Olo upgrades without logging out. `olo-session` (what the login
screen runs) owns the Wayland socket (`wayland-N`) and the IPC socket and
hands both to app_server, so clients never see the sockets disappear.

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
  window returns to its place (same pid, new client: `layout.rs`).
* The proxy shows programs only the globals it can decode
  (`wlproxy/src/interfaces.rs`); add a protocol there before an outside
  program can use it. Set `OLO_WLPROXY_DEBUG=1` for the session
  (`tsession` passes it on) to log every replayed request. Set
  `OLO_NO_WAYLAND_PROXY=1` to have app_server take `wayland-N` itself.
  If the proxy keeps crashing, the session does that on its own.
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
  windows are remembered in memory, up to 200). The place is claimed at
  the window's first commit, so its first configure already has the old
  size, and applied when it maps. Everything else is a new window and
  opens on the display in use (`new_window_near`: beside the app's
  focused window, else under the pointer), including a running app
  opening a window again.
* Kit apps notice the lost connection and reconnect (for up to 20 s). An
  app whose binary didn't change keeps everything and recreates its
  windows exactly (at their current sizes: a panel's last `cx.resize`
  too, since apps like Deskbar only resize when their height changes).
  One whose binary changed calls `App::save_state`,
  re-executes itself (same pid and arguments), and reads the result with
  `cx.restored_state()` at start-up. **Every app with windows the user
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
`$XDG_RUNTIME_DIR/olo-upgrade-<display>.json`; Deskbar polls that file
and shows a green arrow in its tray. Clicking it asks first. Accepting,
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
reload. Test it with
`TSESSION_SESSION_EXE=<install>/bin/olo-session
OLO_UPGRADE_FROM=<tree> tsession start NAME --session --shell` and
`touch` a program in the tree.

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
headless app_server (GPU, offscreen, 2560×1600 device pixels at 2×, i.e.
1280×800 logical) and drives it:

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
$T click mine X Y [right]      # logical coordinates (screenshot px / 2)
$T tap mine X Y                # press and release in one go, as a touchpad tap
$T drag mine X1 Y1 X2 Y2
$T type mine "hello"           # $T key mine 29 30  → Ctrl+A (evdev codes)
$T theme mine dark
$T stop mine
```

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
* Tracker (`tracker.rs`) sets `Settings::tracker` (the view new folder
  windows open in); its icon is Tracker's own, as in Haiku.
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
  (21:9), some with a `-dark` version, and previews in `thumbs/`. The
  setting holds `pack:NAME`, and `wallpapers::resolve` gives the file
  for a screen's shape (2:1 or wider takes the ultrawide one) and the
  effective theme, so it follows dark mode (Auto's schedule too) live.
  It is installed into `~/.local/share/olo/wallpapers` by
  install-session.sh and by upgrades (cloned on btrfs), and found in
  the recorded build tree's `data/` too (`data_dirs`). Choose… opens a
  FilePanel (picture types only) as a child window for your own file;
  its text field follows the setting when it changes elsewhere. app_server draws the wallpaper (`wallpaper.rs`):
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
  from the arrangement and offers only mode, refresh and "Use as".
* Memory buffers drawn by app_server (twitcher, wallpaper, identify) must
  pass the full buffer as `src` to `MemoryRenderBufferRenderElement::
  from_buffer`; without it the buffer counts as logical pixels and is
  drawn at twice its size on a 2× screen.
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
do: Window Arrangement (Tile All, Smart Tile, Zoom All, Cascade All, Fan
Out, Cascade / Fan Out Others, Undo), Workspaces (go to one, Move Window
To, Workspaces preferences) and Show or Hide All Windows. Each is
`arrange:` and a shortcut id, sent as `Request::RunShortcut` naming the
window that last had the focus (`last_focused`: opening the menu takes
the keyboard, so app_server sees no focused window by then).

Twitcher (⌘Tab, ⌘\`, `twitch_step`) shows each window it passes by
bringing it forward (a minimized one unminimized), but keeps the stacking
from when switching began (`Twitch::stack`): passing on puts the last one
back in its place (minimized again if it was), so when ⌘ is let go only
the chosen window has moved.

## Desktop shortcuts and tiling (olo_protocol::shortcuts, app_server keymap.rs, shell/tiling.rs)

app_server's own shortcuts are one table, `olo_protocol::shortcuts::SHORTCUTS`:
each a `Shortcut` (tile the focused window into a half, quarter or third,
zoom, back to its own size, center, to the next display, tile or cascade
all windows on the display, show the desktop, workspaces, Find, lock,
caffeinated, screenshots), a group, a title and default keys written
"Shift+Cmd+Left" (`Chord`; Cmd is ⌘). `Settings::shortcuts` keeps only
changes (an empty list turns one off), so new shortcuts get their defaults.
`shortcuts::assign` gives one shortcut a chord and takes it from any other;
the `defaults_never_clash` test keeps the table clean. The keymap looks
shortcuts up before its own ⌘ translations; Twitcher's ⌘Tab / ⌘\` stay
fixed. To add one: a `Shortcut` variant, a row in `SHORTCUTS`, its case in
input_handler's `shortcut`.

Preferences ▸ Keyboard ▸ Shortcuts lists them; clicking one sends
`Request::RecordShortcut`, and app_server takes the next key with its
physical modifiers (only it can tell ⌘ from Ctrl) and answers with
`Event::ShortcutRecorded` (Esc cancels, ⌫ clears; it gives up after 30 s).

Tiling (`shell/tiling.rs`) works on the display's usable area (zoom's: no
Deskbar or panels) less `Settings::tiling.margin`, with `tiling.gap`
between windows. A tiled or zoomed window remembers its own frame
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
the arrangement takes their windows along (`fixup_positions`). When displays come
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
the display, or unzooms them all if they all are. Window to Workspace 1–9
(⌃⇧⌘1–9) moves the focused window there and follows it. Cascade Others and Fan Out Others (⌃⇧⌘\,
⌃⇧⌘/: `keep`) arrange all but the focused window (or the one the
palette or Deskbar's menu names), which stays where it is, in front and
focused (`windows_to_arrange`, `bring_back`). Every
arranging command (Tile All, Tile App, Cascade, Fan Out) first calls
`remember_arrangement`, which keeps each window's frame, tab offset, tile
and part (`Placed`, in `AnvilState::arranged`); Undo Window Arrangement
(`undo_arrangement`, ⌃⌘Z) puts them back and remembers what it replaced,
so undoing again redoes. Agents run both with
`{"type":"run_shortcut","id":"fan-out"}` / `"undo-arrangement"`.

Roll Up (`toggle_rolled`, ⌃⌘M, the tab's menu, the palette, or a
double-click on the tab, which used to hide the window) leaves just the
window's tab: `Decoration::rolled` makes the element's bbox the tab, draws
only the tab, and passes the pointer through everywhere else. It is in
`WindowInfo::rolled` and kept across a reload (`layout::Placement`).
Agents roll with `{"type":"run_shortcut","id":"roll-up","window":ID}`.

Window groups (shell/groups.rs): an app's top-level, decorated,
resizable windows on the same workspace share one frame, with a tab
each laid side by side along the top (`tab_bar`: each tab as wide as its
title wants, or all an equal share if they don't fit, so tabs never
overlap; `Decoration::tab_width`, `tab_offset`). `sync_groups` runs every
`tick`: the front-most member leads, newcomers take its frame
(`join_group`), and the group is raised together so clicking a tab
brings that window to the front of the frame. Moving, resizing, rolling
up and tiling (one cell per group) act on the whole group;
`behind_in_group` windows are skipped by the arranging commands. A
window leaving a group gets its own tab back and steps out by `CASCADE`
so it isn't hidden. Grouping is on for every app; the ones a person
turns off are `TilingSettings::ungrouped_apps` (app ids), toggled by
Group Windows in Deskbar's team menu and in Tracker's menu for an app
(`olo_kit::settings::groups_windows` / `set_groups_windows`).

Other programs arrange windows through two requests: `Arrangement`
answers the active display's connector, its tiling area (global logical
pixels) and its windows on this workspace, front first, with frames;
`Arrange { windows: [Placement { id, x, y, w, h }] }` puts the listed
ones in those fractions of the area (`arrange`), as Tile All does its
cells: undoable, and tiled as `Tile::Cell` so their edges move together.
Smart Tiling (see "AI features") is the first user.

## Keyboard hints (app_server hints.rs, Preferences ▸ Keyboard)

Opt-in (`KeyboardSettings::hints`). Tapping Control and Shift together
(Caps Lock counts as Control when it is one), with nothing else and
within `HINTS_TAP`, is the keymap's `Action::Hints`: a yellow key cap
with a letter goes on everything that can be picked, as Vim's
EasyMotion does. Typing a letter picks it; Backspace goes back a level;
Esc, a click or any other key (which then does what it does) puts them
away; tapping ⌃⇧ again does too. Letters are by key position, as on a
US keyboard, home row first (`LETTERS`).

What can be picked comes from the kit's trees (`{"tree":true}`, asked on
worker threads, 500 ms at most): the focused window's controls, the
other windows (by their tabs; picking one brings it forward and letters
its content next) and every layer surface (Deskbar, the desktop's icons,
notifications). `build` turns a tree into `Node`s: what is clickable,
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
(`Request::SetCaffeinated`, the Deskbar cup) and apps holding a
zwp_idle_inhibit inhibitor count as activity. `Request::PowerStatus` and
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
displays turning off ends it.

Test headless: `tsession ipc NAME '{"type":"start_screen_saver"}'`, or set
`screen_saver.module` with get/set_settings first.

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

## The touchpad (app_server udev.rs, momentum.rs)

Preferences ▸ Touchpad goes into libinput (`configure_pointer`), with the
rest set as a Mac trackpad behaves: a two-finger tap is the secondary
click, no drag lock, no middle-click emulation. Scrolling has momentum, as
on a Mac (`momentum.rs`): when the fingers lift (libinput's scroll of 0)
while moving faster than 0.3 px/ms over the last 80 ms, app_server holds
the stop back and keeps sending finger scrolls every 8 ms, slowing by
0.997 a millisecond, then sends the stop; fingers resting again (a hold
gesture), pointer motion, a click, a swipe or pinch, or a wheel stop it.
Clients thus see one continuous finger scroll (GTK adds no kinetic scroll
of its own). Kit scroll views have egui's own smoothing. Test with
`tsession flick NAME DY` (ten scroll steps, then the lift);
`tsession scroll NAME 0 0` is a lift on its own.

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
notch the same 48), so gets them ×0.4. Measured headless: a page whose
title shows `scrollY` with rows every 10 px (Chrome), and a numbered
file in gnome-text-editor (24 px lines); `tsession scroll NAME 0 N` (no
lift) and `tsession wheel NAME 1`. Re-measure after a toolkit update.

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
back; Save writes `screenshotN` and quits; Copy to clipboard offers
`image/png`. Settings are in `screenshot.json`. `--silent [--window [ID]]`
saves straight into the folder with no window: app_server runs it for ⌘⇧3
and Print Screen (the screen) and Alt+Print Screen (the shortcut's target
window by id: the active one, or the one the palette was opened over,
since the palette is active by the time it runs).
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
JSON) go in the `user.olo.styles` attribute, alignment and wrapping in
`user.olo.alignment` / `user.olo.wrap`, so other programs see plain
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
shells running; the next Terminal opens a window on each one not shown
elsewhere (a launch with arguments opens a new one as well). Closing a
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

**Tabs.** A window holds one or more shells (`TermWin::tabs`, each a
`Tab` with its own `Session`); its strip (the kit's `TabStrip`) shows
once there are two, and the window grows or shrinks by the strip so the
grid stays the same. Every tab has the window's grid, so a program in a
hidden tab lays itself out right; a hidden tab isn't drawn (its Wakeup
waits until it shows), and its bell marks it with a dot. Messages from
the emulator carry the shell's id, not the window's, so a tab can move:
**Tabs ▸ Move Tab to New Window** (or the tab's context menu). New Tab
(⌘T) starts in the current tab's folder; ⌘W closes the tab, Close Window
the lot (asking if anything is running). ⌘[ / ⌘], ⌘1…⌘9, Ctrl+Tab /
Ctrl+Shift+Tab and Ctrl+Page Up / Page Down go through them (the shell
never sees those). `olo-terminal --tab [-w DIR]` opens a tab in the
Terminal window last in front (Tracker's Open in Terminal does), a window
if there is none. `save_state` keeps each window's tabs in order and
which one shows.

**Groups** (`src/groups.rs`). A window can be a named group with a colour
(Tabs ▸ Group ▸ Name Group…, or click the group's name in the strip): the
name leads the strip in its colour, every tab carries a band of it, the
window's title is "name — tab", and the strip shows even with one tab.
Save Group keeps the group and each tab's folder in
`terminal-groups.json` (config dir); Open Saved Group brings its window
forward, or opens one with a shell in each folder; Forget Saved Group
drops it. Show All Tabs (Tabs menu or the group's menu, ⌘⇧D) shows
every tab of the window at once, each a live pane with its title over
it (`tile` picks the columns that keep panes nearest a terminal's
shape, the last row spread across), each pane's shell sized to its pane;
clicking a pane makes it the one typed into, marked with the group's
colour (or the focus blue) as the strip marks it, and the strip follows.
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
(see keymap.rs); Ctrl+Shift with a menu shortcut's key
(`view::SHORTCUTS`, kept in step with the menus) is Terminal's, and any
other goes to the shell as Ctrl+key, so ⌘R searches history like Ctrl+R. ⌥←/⌥→ send ESC b / ESC f
(a word back/forward in readline, zsh and fish), except to a program
using the kitty keyboard protocol (Claude Code), which gets the
protocol's `CSI 1;3 D` / `C` and reads ESC b as Escape then b.

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
  and our own `x-olo-type`. Each note has a `Kind` after Haiku's
  BNotification: information, important (`im.*` and `call` categories),
  error (critical: stays) and progress. The timeout: -1 is 6 s, 0,
  errors and notes waiting for a reply stay until dismissed. The UI
  answers with `Bus::closed` (reason 1 expired, 2 dismissed, 3 closed by
  the app), `Bus::action` after `Bus::activation_token`, and
  `Bus::replied` (`NotificationReplied`). If another service owns the
  name it waits and retries every 30 s rather than taking it. (zbus's
  `Builder::name` treats "taken" as success; `serve` checks the
  RequestName reply itself.)
- main.rs: one layer surface (Top layer, anchored bottom right, flush
  with the edge) opened for the first card and closed with the last,
  resized to the stack. Cards are `metrics::DESKBAR_WIDTH` wide, so they
  stack in the Deskbar's column, usually free space. Up
  to four cards show, grouped by app under a title row with the app's
  icon and name (and a close box for all of a group's), oldest first; the
  rest wait. Important notes wear a blue edge, errors a red one; progress
  shows a `StatusBar`; links in the body open with `open::open`. A card
  shows the picture sent (24 px) beside its text; without one the text
  takes the width, the group's title row already showing the named icon
  (ours for common theme names, `icon_names`), the app's desktop-entry
  icon, or info/warning. Action buttons share the width when they must.
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
  history.json` (the newest 200; saved atomically off the UI thread).
  Every note not `transient` goes in, unread unless the History window
  is open; an expired card stays "open" there (its actions still work)
  until the app closes it. After a restart nothing is open, and ids carry
  on after the history's.
- history.rs: the History window (`--history`, or Deskbar's bell), newest
  first in one of four views (View menu, ⌘1–⌘4, `view` in
  `notifications.json`): *List* under day headings, *By App* under
  folding app headings (with Dismiss per app), *Cards* in a grid whose
  rows are as tall as their tallest card, *Conversations* with the same
  app and title stacked into one with a count. A row is two lines: a
  16 px icon, the title in bold with the app and time on its line, one
  line of body; a click selects (the row opens to its whole body, the
  rest of its conversation and its actions as buttons, the default one
  always Open, or Show APP once it closed, then Dismiss, which forgets it
  and the rest of its conversation), a double click or Return runs the default action while
  the note is open, else brings the app forward. × or Delete forgets
  one; Clear All forgets all but those on screen. Opening the window
  marks all read, but what was unread keeps a blue dot until the window
  closes or it is selected. The search field (⌘F) takes every word
  against app, title and body; Escape clears it. For agents each note
  is a `ListItem` named by its title (with `row`/`rows`), its text as
  labels. The window, search and folds come back after an upgrade
  (`save_state`).
- Settings: `olo_protocol::NotificationSettings` in
  `notifications.json` (config dir): Do Not Disturb (`dnd`, `dnd_until`,
  a daily `schedule` in local minutes), banner `seconds`, and per-app
  `banners` / `sounds` / `history` by app name. The service re-reads the
  file within a second of a change (Preferences ▸ Notifications and
  Deskbar write it). While quiet nothing pops up or sounds but errors.
  It publishes `NotificationState` (`unread`, `quiet`) to
  `$XDG_RUNTIME_DIR/olo-notifications-$WAYLAND_DISPLAY.json`, which
  Deskbar's bell reads twice a second (the badge; struck through while
  quiet; right-click for Do Not Disturb).
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
  (`List` → JSON, `Invoke`, `Dismiss`, `ClearHistory`, `ShowHistory`,
  `SetDoNotDisturb`), and the same from the command line (cli.rs):
  `olo-notifications [--bus A] --list [--json] | --history |
  --invoke ID [ACTION] | --dismiss ID | --clear | --dnd on|off|MINUTES`.

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

⌘Space (`Shortcut::Palette`; Find moved to ⌥⌘Space) or a tap of ⌘ (if
chosen in Preferences ▸ Keyboard) has app_server start
`olo-palette --toggle`; the running copy (it stays resident, found
through `instance`) takes the toggle, so it opens at once from the second
time on. It opens a full-screen transparent overlay layer that takes the
keyboard, with the BeOS panel a fifth of the way down: a click outside
the panel, Escape or ⌘Space again
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
  is `Request::Show`), files under home by name (indexed by an `ignore`
  walk on start and every five minutes, at most 400,000; matched on their
  own thread for the newest query only), sums (`calc.rs`; Return copies)
  and, last, a web search (Google) in the web browser.
* **Ranking** (`rank.rs`): every word typed must match the title (start,
  word start, anywhere) or the item's keywords, or as initials ("nw");
  then by kind (apps before files) and by how often and how lately each
  thing was chosen (`~/.config/olo/palette.json`). An open window is
  also found by its app's name and comes just before the app itself
  (switching beats opening another), the one in front last. Nothing
  typed lists what was chosen lately.
* **Keys**: ↑/↓ and Page Up/Down move, Return chooses, the pointer
  hovers and clicks. Actions run on a worker thread.
* **The window menu** (`window_menu.rs`): a right-click on a window's
  yellow tab has app_server (`wm.rs` `window_menu`) start
  `olo-palette --window-menu ID X Y OUTPUT`, the point on that output.
  A worker asks app_server for the window's title and workspace, the
  workspace count and the displays; the palette then opens a clear
  full-screen overlay on that output and pops an ordinary kit menu at
  the point: Hide, Zoom, Back to Its Own Size, Center, Tile ▸ (halves,
  quarters, thirds), Move to Workspace ▸, the next/previous display
  (with more than one), Send to Back (BeOS's right-click on the tab,
  `Request::SendToBack`) and Close. Its items are the shortcuts', done
  through `RunShortcut { id, window }`, so the menu, the keys and the
  palette always agree; agents read and choose from it as from any
  popup. Choosing, a click outside or Escape puts the overlay away.

## Other toolkits' programs (app_server toolkits.rs, state.rs; olo_theme::toolkit)

Programs not built on the kit take Olo's look as far as their toolkit
allows. `toolkits.rs` publishes the palette where GTK 3, GTK 4,
libadwaita and Chrome read it (GSettings through dconf, the Olo and
Olo-dark GTK themes, GTK 4's user stylesheet, BeOS's close-left,
zoom-right title buttons); see its module comment.

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
outline, or `--json`), `menus` and `invoke` (a window's app's menu bar
and choosing from it, `commands::menus`/`invoke`), `click`, `move`,
`down`/`up`, `drag`, `scroll`, `wheel`, `key` (chords as people write
them: `Cmd+S`, `Enter`, `Esc`; parsed by `olo_protocol::Chord` with
aliases) and `type` (US keyboard), all as `Request::Synthetic`, in the
screen's logical pixels. `stop` ends the session, and with it the
programs on it. `olo-screen --help` lists it all.

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
outlines a quarter-second apart the same) and `scroll-to` (wheel to the
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
watch NAME`): the screens in a list and the chosen one live beside it,
about eight frames a second with the agent's pointer drawn in (a worker
asks for `format: "rgba"` pictures and reads them back; the window never
waits on a screen). Take Control (⌘K) steps in: the person's pointer,
wheel and keys over the picture go to that screen through another worker
(keys by their evdev codes, Ctrl, Shift and Alt pressed and let go as
each key event says), with a ring in the keyboard-navigation colour
round the picture, until Stop Controlling. New Screen starts one; Stop
Screen… ends one after asking. A viewer running on an agent screen
leaves that screen out of its list. While any screen runs (other than
the one a Deskbar is on), Deskbar's tray shows a monitor with a pointer,
and their count when there are several (`status::agent_screens` reads
the same listing every second, judging entries as `Screen::alive` does);
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
only `logins`, `fill`, `check`, `save`, `unlock` and `show` on to
Passwords over the keychain socket (`{id, request, start}`; `start`
starts Passwords and waits out its unlock) and `{open: true}`. The
extension: a field's context menu has Passwords ▸ each login ▸ Username,
Password, One-Time Code (when it has one), Fill Login, and Open
Passwords (Unlock Passwords… while locked); the menu is rebuilt for the
frame a field gets the focus in, and a fill goes only to that frame and
only while it is still at the origin the login was asked for. The
toolbar button lists the page's logins (Enter or a click fills one). A
password sent with a form (submit, a button, Enter) is checked, and a
new or changed one offered in a notification (Save/Update, Not Now; the
pending login is kept in `storage.session` only). Passwords names the
waiting app from the keychain caller's program (`Incoming::from`). Test
with a throwaway `--user-data-dir` (DevTools needs a non-default one:
`--remote-debugging-port`), the fake 1Password and a localhost page; the
fake's "Olo Test Site" login (`http://localhost`, jane) has a TOTP.

**Chrome's widget tree, for agents** (`extension/tree.js`, `host.rs`).
The extension (named "Olo": it does more than passwords) connects to
its host when its worker starts and stays connected (reconnecting after
10 s), so a host runs beside every Chrome. The host answers a kit app's
`tree` socket for its parent, Chrome's browser process (the Wayland
client app_server names in `WindowInfo::pid`), with
`commands::serve_trees`: each question goes to the extension as
`{tree: n}`, which injects `tree.js` into each window's active tab (top
frame) and answers `{tree: n, windows}` within `TREE_WAIT`. `tree.js`
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
journal stream), and Log Viewer reads that journal natively.

- journal.rs: systemd's journal file format (JOURNAL_FILE_FORMAT.md): the
  header, entries through the chain of entry arrays, fields in data
  objects (zstd, xz or lz4; the regular and the compact layout). Data
  objects are cached by offset (most are shared between entries). A
  file still being written is read on from where it was left, and
  across journald rotating it (the open file is finished, then the new
  one opened). `user_files(uid)` finds `user-UID.journal` and its
  archives in /var/log/journal (or /run/log/journal), oldest first.
- reader.rs: on a worker thread, every file in batches, then the active
  one polled every ½ s; `Update::Lines` carries new lines and more text
  for lines sent before.
- model.rs: an entry as a line: the program (`_EXE`'s name, else
  `_COMM` made whole from a longer name seen, else the identifier;
  Chrome's lines, which Google's launcher passes through `cat`, as
  chrome), the level and text without terminal colours and without
  `tracing`'s time and level (or Chromium's `[pid:tid:time:LEVEL:file]`),
  GLib's `-WARNING **` and panics recognised, and Olo's own stderr
  lines Info unless they say otherwise (the session's stream files all
  of stderr as warnings). Continuation lines (a pretty debug print, a
  backtrace) join the line before from the same process. `Query`: time
  (last 10 minutes, hour, today, since startup, all), minimum level, one
  program or Olo's or all, and text (in the message or the program).
- highlight.rs: syntax highlighting as spans: the target `tracing` puts
  first, `key=value` keys, strings, numbers, paths and URLs, type names,
  trouble words; the list and the detail pane colour them from the
  palette.
- main.rs: Find and the three filters above a `ColumnListView`; the
  selected line in full (highlighted, selectable) below; Live keeps the
  list at its end as lines arrive; Show in Context (⌘K, or a line's
  context menu) shows every line around one, and Back returns to the
  filters. The filters are kept; Find isn't.

For agents, the same query from the command line, reading the journal
directly: `olo-logs --print [--since 10m|2h|1d|today|boot|all] [--level
debug|info|warning|error] [--program NAME | --all-programs] [--find TEXT]
[--json] [--follow]` (JSON lines: time in UTC, program, pid, level,
message). Tests: `cargo test -p olo-logs` (synthetic journal files
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
(`olo_system::secrets`, a Secret Service client; PAM unlocks the
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
opens it), which listens on `127.0.0.1:17891` (`OLO_POOL_PORT`).

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
  is the state without sign-ins.
- **Order** (`schedule.rs`): earliest-expiry-first. The account whose
  usable quota resets soonest serves, since quota that resets unused is
  lost; ties go to the older account, then the id. The account serving
  keeps serving unless another resets more than five minutes sooner.
  Windows can be model-scoped (Weekly Opus): a Sonnet request isn't
  held up by Opus's meter.
- **Usage** is read from the answers as they stream through: only
  `usage` objects (SSE lines containing "usage", or a JSON body's), the
  content discarded as it passes. Quota comes from every answer's
  rate-limit headers and, each minute, the providers' usage reports.
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
  `ANTHROPIC_BASE_URL`, a placeholder `ANTHROPIC_AUTH_TOKEN`, an empty
  `ANTHROPIC_API_KEY` and `CLAUDE_CODE_MAX_RETRIES` in Claude Code's
  `settings.json` `env`; "Codex uses the pool" adds an `olo_pool`
  model provider to `config.toml` (toml_edit, so comments stay) and
  selects it. What was there before is kept in `pool-clients.json` and
  put back when unchecked.
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

Tests: `cargo test -p olo-token-pool` runs the gateway against a fake
provider. For the window, start a session with a fixture `--home`
holding fixture credentials, a fake upstream
(`OLO_POOL_UPSTREAM=http://127.0.0.1:PORT`: its `/oauth` takes
refreshes), `OLO_POOL_PORT` and `OLO_POOL_KEYFILE`, and a fake
`claude` or `codex` on `PATH` for separate sign-ins. Never real accounts.
