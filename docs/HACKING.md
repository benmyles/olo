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
| `crates/session` | `tabula-session`: starts and supervises app_server, Deskbar and the desktop; in-place reload and upgrade (see below). |
| `crates/protocol` | JSON-lines IPC between app_server and our apps (`Client`, `Request`, `Reply`, `Event`, `Settings`). |
| `crates/theme` | `Palette` (light/dark), BeOS tint helpers, decorator metrics, font lists. |
| `crates/kit` | The Interface Kit: Wayland client platform, GPU rendering (egui on wgpu/Vulkan), menus, BeOS widgets, icons, `.desktop` entries. **All apps are built on it.** |
| `crates/xdg` | `tabula-xdg`: freedesktop plumbing shared by app_server and the kit (re-exported there): data dirs, `.desktop` entries, icon lookup/decoding (`load_icon`), our own apps (`apps::our_app`), MIME types (`mime::for_path`), opening files and URLs with the right app (`open::open`, `open::handler_cached`: never `xdg-open`; Tabula's apps first, except that links and web pages go to the browser chosen in mimeapps.list, else `tabula-chrome`), `find_program`, `spawn_detached`. No egui. |
| `crates/system` | `tabula-system`: the system over its own protocols, in process: logind (`power`: reboot, power off, suspend, inhibitor locks), NetworkManager (`network`: status, join including 802.1X, forget), the sound server (`audio`: volume, mute, `watch` change feed), dconf writes, the D-Bus/systemd activation environment. All blocking: call from worker threads. |
| `crates/preview` | `tabula-preview`: native previews of every file kind (text, images, PDF, audio, video, archives, office documents, fonts). Used by Tracker's Quick Look (`Preview::new`, images capped at 3200 px) and the Viewer (`Preview::new_zoomable`: full resolution up to the GPU texture limit, zoom and pan, `zoom` / `set_image_scale`). Images carry halved copies (`load::halvings`) because egui-wgpu makes no mipmaps. Video (`video.rs`, H.264 in mp4 or Matroska and AV1 in Matroska/WebM, with `media.playable` set): a worker demuxes and decodes (OpenH264, or rav1d through `tabula_images::av1::Decoder`) up to six frames ahead, giving each picture the earliest outstanding presentation time (OpenH264 returns display order; it must be made with `Flush::NoFlush`, as the crate's default flush after every packet breaks B-frame reordering and every B-frame then fails), mp4 times shifted by the track's edit list; `Video::tick` shows frames against a clock that follows the sound's reported position when an `audio::Player` is playing it and runs alone otherwise (no sound server in tests, silent files). Seeks go to the key frame before the target (Matroska: scans back for one, further each time) and drop pictures before it; a paused seek still shows its frame. `TABULA_SAMPLES=dir cargo test --release -p tabula-preview video -- --ignored` checks seeking on test.mp4/mkv (one key frame) and gop.mp4/mkv (one a second), made with ffmpeg's testsrc2, plus av1.webm (svt-av1, Opus sound) and bframes.mp4 (any x264-made H.264 High clip with B-frames; `b_frames` checks every picture comes out, in order); `keeps_up` needs av1-1080.mkv (1080p AV1) to decode at well over 30 frames a second (72 on the M2 Max; the YUV to RGB fast path is fixed-point with tables, checked against the float path by `yuv_fast_path`). Audio decodes with symphonia plus `opus.rs` (opus-decoder, OpusHead parsing and pre-skip trim) for Ogg Opus. Picture editing (`edit.rs`): an `Editor` keeps a list of `Op`s (rotate, flip, crop, resize) with undo/redo and a saved mark, and a worker that loads the full picture once, applies new steps to its last result, sends `EditEvent::Shown` (downscaled for the screen, with halvings) for `Preview::show_edited`, and writes PNG, JPEG (quality 92), WebP, BMP, TIFF or GIF atomically (`write`, through a hidden `.name.saving` file); `edit::editable` says which files it can write back. `CropTool` is the crop overlay (drag a frame, move it or its edges; `Preview::image_rect` gives where the picture is drawn, `set_still` stops pan and zoom while it is up). The Viewer's Image menu and File ▸ Save / Save As / Revert use it; Quick Look's footer has Rotate Left / Right (⌘L / ⌘R), which save at once. |
| `apps/<name>` | One crate per application, binary named `tabula-<name>`. Picked up by the workspace glob. |
| `vendor/` | Patched copies of dependencies (`[patch.crates-io]`), each explained in `vendor/README.md`; drop them when upstream has the fix. |
| `data/icons` | Icon set, pre-scaled to device sizes (`data/icons/<px>/<name>.png`). |
| `data/session` | `tabula.desktop` for the login screen (installed by `tools/install-session.sh`). |
| `tools/` | `install-session.sh` (release build → `~/.local`, session entry → `/usr/local/share/wayland-sessions`), `dev/tsession` (headless UI testing), `genicon.py`/`genicons.sh`/`mkicons.py`/`iconsheet.py` (icons, see [ICONS.md](ICONS.md)). |

Build: `cargo build -p tabula-<app>` (plus `cargo build --bin tabula-app-server` once, for headless testing; as root set `CARGO_HOME=/home/ben/.cargo` to share the registry) (repo is owned by `ben`; when running as
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
* **Single instance** (`tabula_kit::instance`): call `instance::forward(app_id,
  &args)` first in `main` (returns true if a running copy took the
  arguments; exit then), and `instance::listen(app_id, cx.proxy())` in the
  init closure; forwarded launches arrive in `App::message` as an
  `instance::Launch { args, cwd }`.
* **Following a setting** (`tabula_kit::settings`): `current()` reads
  app_server's `Settings` once (a quick local round trip; fine at
  startup), `watch(f)` calls `f` on a thread now, after every
  `SettingsChanged` and after app_server restarts; send what you need
  through `cx.proxy()`. Tracker follows `tracker.default_view` (the view
  new folder windows open in, column view by default) this way.
  `update(edit)` changes them from an app (read, `edit`, `SetSettings`, on
  a thread), and `watch_power(f)` follows `PowerStatus` the same way
  (the screen locking, the displays sleeping).
* **Menus for the command palette** (`tabula_kit::commands`, automatic):
  every kit app answers on `$XDG_RUNTIME_DIR/tabula-menus-<pid>.sock`
  with the menu bar of its last active window, flattened
  (`commands::menus(pid)`: path, label, command, keys as shown, enabled,
  checked), and chooses an enabled one as if from the menu
  (`commands::invoke(pid, command)`). Both block for at most a second;
  call them off the UI thread.
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
* Taps: a touchpad tap brings a press and its release together; the kit
  holds such a release over to the next frame (`input::split_taps`), so
  egui sees the button down for a frame and press-and-hold controls
  (menus, scroll arrows, drags) work from taps too.
* The menu bar opens a menu on press, as on BeOS, reading the press from
  the frame's events (a touchpad tap brings press and release in one
  frame); pressing the open menu's title again closes it, but a menu that
  slid open as the pointer crossed the bar stays open for that press.
* Embedded surfaces (another renderer inside a window: NetPositive's web
  engine): `cx.embed(window)` makes a `wl_subsurface` of the window
  (desynchronised, empty input region, with a viewport) and returns an
  `EmbedId`; `App::embedded(cx, &Embedded)` then hands over its raw
  display and surface handles and the scale, for a GL or Vulkan context of
  the renderer's own. It is called again after a reconnect (new handles)
  and when the window's scale changes (same surface; just resize). Place
  it each frame with `cx.place_embed(id, rect)` (logical, as egui's; it
  sits above the window's own drawing, which shows through until the
  renderer's first frame). Input over it reaches the window as usual, for
  the app to forward. `App::embed_lost(cx, id)` comes before the surface
  goes (window closing, connection lost): drop everything that uses the
  handles there. `cx.remove_embed(id)` removes one early.
* Session lock (the lock screen only): `cx.lock_session()`, then
  `SessionLock::Locked(windows)` in `App::message` (one window per output,
  the first is primary; kept up to date as outputs come and go) and
  `cx.unlock_session()`. See "The lock screen".

### Drag and drop (`tabula_kit::dnd`)

Over Wayland's data device, so it works within a window, between
Tabula's apps and with any other Wayland program. As in BeOS (the Be
Book's `DragAndDrop.md`), a drag carries the same data in several types
and offers actions; where it is dropped picks the type and the action.

* **Dragging out:** on an egui `drag_started` (the button still down, so
  that frame), `cx.start_drag(window, DragData)`. `DragData::files(&paths,
  Actions)` carries `text/uri-list` (file:// URIs: file managers,
  browsers, editors) and the paths as plain text (terminals);
  `DragData::text(s)`; `.with(mime, bytes)` adds a type only Tabula knows,
  first; `.icon(DragIcon)`; `.from_folder(dir)` names the folder the files
  are in (`Hover::from_folder(dir)`), so Tracker refuses dropping them back
  where they are, from any app. Viewer and Quick Look drag their file out
  from the picture when it fits (`Preview::take_drag_out`: a drag that
  doesn't pan), Quick Look also from its footer's icon, Get Info from its
  icon.
  Tracker's folders are spring-loaded: a drag resting on one for
  `view::SPRING` (1 s) opens it (`item_drop` times it; `take_spring`).
  Deskbar takes files dropped on a running application's row and opens
  them with it (`open::open_with(handler, paths)`: our apps get them all
  as arguments, a desktop entry at once with %F/%U or once each). `icons::drag_icon(ctx, icon, size, count,
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
* `Action::Ask` (a secondary-button drag, as on BeOS): the place it lands
  shows a menu (Tracker: Move Here / Copy Here / Create Link Here).
* `RUST_LOG=tabula_kit=debug` traces drags (start, enter, the offer told,
  drop). Headless screenshots show the drag picture; `tsession down X Y
  [right]`, several `move`s, then `up [right]` drives one.

Tracker (`view.rs`: `Drag`, `DropTarget`, `take_drop`, `item_drop`):
files drag from every view and the desktop and drop onto folders (in a
view or on the desktop), into a folder window (anywhere in it; in the
column view, a column or past the last one), onto the Trash (moves them
there) or into another app. Within one file system they move, across
file systems they copy, ⌥ copies anyway; Tracker knows which before the
drop from `application/x-tabula-device-N` in its own drags, and only its
own drags are moved (another program might delete what it was told was
moved). Text dropped on a folder becomes a clipping file named after
its first words. The desktop keeps icons where they are dropped
(`~/.config/tabula/desktop.json`, `icons`: name or path → offset of the
cell from the screen's top-right, in logical px; things dropped onto it
from elsewhere land where they fell; a rename carries the place over);
the rest fill the grid around them, and Clean Up in its menu puts every
icon back on the grid. Terminal types dropped files as their
shell-quoted paths (and text as if pasted), bracketed when the program
asked for it: drag an image into Claude Code there.

### Widgets (`tabula_kit::widgets`)

`StringView` (a label; `.dim(true)` for hints, `.wrap(true)` for a paragraph, `.color(c)`), `Button` (`.default(true)` for the Enter button), `CheckBox`, `RadioButton`,
`Button::pushed(true)` for a toolbar toggle that stays down,
`TextControl` (`.label("Name:").divider(x)`), `TextView` (`.highlight(&f)`
colours it: `f(text, font, colour)` returns a `LayoutJob` in sections, a `widgets::Highlighter`,
as Log Viewer's syntax highlighting does), `Slider`
(`.label().value_text().limits()`), `MenuField` (pop-up choice), `BBox`
(etched group box with label), `separator`, `StatusBar` (progress),
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
with `text_job`), `TabView`, `color_well`, `heading`,
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
None if unset or missing). Bytes too many to keep in memory (a download
being gathered) go in `tabula_xdg::scratch_file()`: a nameless file under
`tabula_xdg::cache_dir()`, on disk rather than /tmp, gone once closed.

Menus: `Menu::new("File").item(label, command, Some(Key::N))`,
`.item_shift(…)`, `.separator()`, `.submenu(Menu)`, `.checked(bool)`,
`.enabled(bool)`. Popups are real `xdg_popup`s drawn by the kit. One
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
   bands with `style::selection_band`. Keys as on a Mac menu ("⇧⌘←", each
   symbol on the text's baseline, though they come from a fallback font):
   `style::key_combo(painter, right_center, keys, color)` (and
   `style::shortcut` for a ⌘ menu shortcut).
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
   keeps ⌘Tab / ⌘\` (Twitcher), ⌘ arrows and ⌘⌫ and the desktop shortcuts
   (see "Desktop shortcuts and tiling") for itself; terminals listed in
   `KeyboardSettings::terminal_apps` get ⌘ as Ctrl+Shift so physical Ctrl
   reaches the tty), keyboard focus shown as a navigation-blue underline/frame,
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
* tabula-session's watchdog (`crates/session/src/watchdog.rs`) pings
  app_server over IPC every 2 s. After 10 s without an answer it sends
  SIGABRT (so `coredumpctl debug` has the stack that hung), then SIGKILL
  10 s later, and restarts it like a crash: apps reconnect and a locked
  screen comes back locked.
* Never cache anything about the connection you can't rebuild; never
  replace an installed binary by writing over it (install to a temporary
  name and rename, as `tools/install-session.sh` does).

Install for the login screen with `tools/install-session.sh` (as
yourself; it asks for sudo once). It builds release binaries into
`~/.local/bin` (so upgrades need no root), icons into
`~/.local/share/tabula`, and the session entry into
`/usr/local/share/wayland-sessions/tabula.desktop` (Exec is the installed
path; no TryExec, as the greeter can't look inside a private home).

**Upgrades from the build tree** (`crates/session/src/upgrade.rs`): the
installer records the tree's `target/release` in
`~/.local/share/tabula/build-dir` (`TABULA_UPGRADE_FROM` overrides it).
Every 3 s the session compares the `tabula-*` programs there with its
own. It skips this while cargo holds one of its locks in the tree
(`.cargo-lock`, or `.cargo-build-lock`/`.cargo-artifact-lock` in newer
cargo) or a program is under 3 s old. It lists newer ones in
`$XDG_RUNTIME_DIR/tabula-upgrade-<display>.json`; Deskbar polls that file
and shows a green arrow in its tray. Clicking it asks first. Accepting,
or `tabula-session upgrade` (SIGUSR2), installs them beside the session:
a btrfs/XFS clone (`FICLONE`) or a copy, under a temporary name and then
renamed. Only a new `tabula-app-server` needs a reload, and a reload
closes programs not built on the kit (GTK, Qt, Xwayland), so Deskbar's
alert names them first. Otherwise app_server keeps running: the session
sends SIGURG to each of the user's processes whose program file in the
install directory was just replaced (`/proc/PID/exe` ends in
" (deleted)"). The kit catches it (a self-pipe in `platform.rs`), saves
state and re-executes, as after a reload, and app_server puts its windows
back where they were. A new tabula-session re-executes itself without a
reload. Test it with
`TSESSION_SESSION_EXE=<install>/bin/tabula-session
TABULA_UPGRADE_FROM=<tree> tsession start NAME --session --shell` and
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

**New hardware**: `tabula-app-server --probe-gpus` lists the DRM devices,
their KMS resources and the render node their EGL uses, without
disturbing a running desktop. On Apple silicon the boot GPU (asahi) has
no displays. The display controller (apple-drm) renders through it via
Mesa's kmsro, so a primary GPU without modesetting is skipped rather
than treated as fatal. Under GDM the session's output goes to the journal
(`journalctl -b | grep tabula`). If app_server can't start, the session
gives up after 5 crashes a minute and the login screen comes back;
Ctrl+Alt+Backspace quits the session.

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
$T tap mine X Y                # press and release in one go, as a touchpad tap
$T drag mine X1 Y1 X2 Y2
$T type mine "hello"           # $T key mine 29 30  → Ctrl+A (evdev codes)
$T theme mine dark
$T stop mine
```

sudo resets HOME, so without `--home` a session and its apps use the real
home folder: pass `--home` whenever a test saves files (Screenshot,
Tracker, StyledEdit…). Sessions are always silent (`PULSE_SERVER` points at
a dead socket), so tests never reach the real speakers or volume. To
test sound, run `tabula-fakesound SOCKET` (tabula-preview): it speaks the
PulseAudio protocol, plays nothing, and logs each stream once a second
(bytes consumed in real time, corked or not, loudness); start the app with
`$T run NAME PULSE_SERVER=unix:SOCKET …`.

The headless server renders on the GPU's render node and announces it
in dmabuf feedback, so GL clients (NetPositive's Servo) get hardware GL
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
* Tracker (`tracker.rs`) sets `Settings::tracker` (the view new folder
  windows open in); its icon is Tracker's own, as in Haiku.
* Network (`network.rs`) reads NetworkManager through
  `tabula_system::network` on a worker thread (devices, addresses,
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
  `TABULA_FAKE_NETWORKS=1` (debug builds; pass it with `tsession run NAME
  env TABULA_FAKE_NETWORKS=1 …`) shows made-up networks instead and
  pretends to join, so the panel and the sheet can be driven headless.
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
* Notifications (`notifications.rs`) edits `NotificationSettings`, its
  own file rather than app_server's `Settings` (so no Defaults/Revert): a
  watcher thread reloads it, and the apps from the history, when either
  changes; edits save on a thread (echoes of older files are ignored for
  a moment after an edit). Times follow the clock's 12/24-hour choice.
* Backup (`backup.rs`) exports every Tabula setting and piece of saved
  state (`config_dir()` and `state_dir()`, as `config/…` and `state/…`)
  into one `.tabula-backup` file: a zstd tar with `manifest.json` first
  (format, version, when, each file and its size). Import reads the
  manifest and shows it by app (`Manifest::groups`) with Import / Cancel;
  importing first saves what is there to `state_dir()/backups` (the last
  five; left out of backups), then replaces each file atomically, keeping
  its mode; names outside the two roots are refused, and files the backup
  lacks are left alone. app_server keeps its settings in memory, so the
  backup's `app_server.json` is sent to it as `SetSettings`. Both run on
  a worker thread with a StatusBar and Cancel (import can only be
  cancelled while it saves what is there). `tabula-preferences --export
  FILE`, `--inspect FILE` and `--import FILE` do the same without a
  window; opening a `.tabula-backup` (`mime::SETTINGS_BACKUP`, kit
  `Kind::SettingsBackup`) shows it in the panel, ready to import.
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

## Desktop shortcuts and tiling (tabula_protocol::shortcuts, app_server keymap.rs, shell/tiling.rs)

app_server's own shortcuts are one table, `tabula_protocol::shortcuts::SHORTCUTS`:
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
else its own frame. Moving or resizing it by hand forgets the tile
(`forget_tile`). Moving screens in
the arrangement takes their windows along (`fixup_positions`). When displays come
or go (`follow_main_output` → `rehome_windows`), windows left on none go to
the main display, cascaded and shrunk to fit, the rest are re-tiled,
re-zoomed or fitted where they are, and the pointer is brought back; a
minimized window is fitted when it comes back.

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
through hidraw (`tabula_system::hid_monitor`), matched to their connector
by the EDID maker; `data/udev/70-tabula-displays.rules` (installed by
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
of its own). NetPositive eases a mouse wheel's notch out over a few frames
(`input.rs`); kit scroll views have egui's own smoothing. Test with
`tsession flick NAME DY` (ten scroll steps, then the lift);
`tsession scroll NAME 0 0` is a lift on its own.

**Scrolling** goes the same distance in every program (`scroll.rs`, at
the one place app_server hands a scroll to a client): Tabula's apps
scroll a finger scroll's values as logical pixels and a wheel notch 48.
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

## Terminal (apps/terminal)

The shells don't run in Terminal: they run in **tabula-termd**
(`src/termd.rs`, a second binary of the same package), one per display,
which the first Terminal starts (it daemonizes and holds
`$XDG_RUNTIME_DIR/tabula-termd-<display>.lock`, with its pid in it). It
owns every PTY and keeps its own emulator per shell (the same `io::Feeder`
and image placement as Terminal, without the pixels), so it always knows
the screen. Terminal talks to it over
`$XDG_RUNTIME_DIR/tabula-termd-<display>.sock` (`src/wire.rs`: framed JSON
and raw bytes; one connection per Terminal, `src/client.rs`): it spawns
or attaches a shell, sends input and resizes, and gets the output. On
attach, tabula-termd sends a replay (`src/replay.rs`) that rebuilds the
screen and scrollback in a fresh emulator: both screens when a program
is on the alternate one, the modes it set (cursor keys, mouse reporting,
bracketed paste, kitty keyboard flags…), cursor shape and title. Images
(kitty, sixel, iTerm2) come back as kitty graphics: each image sent once
under the program's id, then put row by row in its cells; tabula-termd
keeps them zlib-compressed (`Store::pack`). The PTY comes along (SCM_RIGHTS) so Terminal can see the
foreground program and its working directory. tabula-termd answers the
program's questions (device attributes) only while no Terminal is
attached.

So: **Quit** (and closing the last window) says goodbye and leaves the
shells running; the next Terminal opens a window on each one not shown
elsewhere (a launch with arguments opens a new one as well). Closing a
window hangs up its shell. If Terminal **crashes**, tabula-termd starts
it again after 2 s (up to 3 times a minute, only while the session and
its Wayland socket are there). An **upgrade** re-executes both in place:
Terminal keeps which shell was in which window (`save_state`), and
tabula-termd hands its PTYs and replays to its new self through an
inherited file (`TABULA_TERMD_RESTORE`); Terminal reconnects on its own.
(A Terminal from before tabula-termd hands its shells' PTYs to its new
self, the old way; the new one passes them on to tabula-termd, ADOPT.)
When the desktop session ends (its pid file, watched with a pidfd) or on
SIGTERM, tabula-termd hangs up every shell and exits; it also exits when
it has no shells and no Terminal for 3 s, or when its runtime directory
has gone. If its socket is deleted from under it, it listens again (so a
Terminal can always find its shells); it only removes the socket while
the file there is still its own. Each screen is kept twice (in
tabula-termd and in Terminal), so a full scrollback costs about twice
what it used to.

**Terminal ▸ Terminal Daemon…** (`src/daemon.rs`) shows tabula-termd's
process and uptime and its shells (whether each is shown here, in
another Terminal, or nowhere; double-click one to show it), refreshed
every 2 s off the UI thread. **Restart** re-executes it in place (shells
kept); **Start Over…** (after an alert) ends it, by its pid, with SIGTERM
then SIGKILL, starts a fresh one, and opens a new window once the old
shells' windows have closed. One that isn't answering can only start
over.

Testing: tabula-termd lives in the session's runtime directory, so a
headless session gets its own. Kill only the Terminal or tabula-termd
you started, found by `XDG_RUNTIME_DIR` in `/proc/PID/environ`.
`cargo test -p tabula-terminal` covers the wire format (with descriptor
passing) and replays (scrollback, `clear`, the alternate screen, modes,
images).

Keys: physical Ctrl goes to the shell. ⌘ reaches Terminal as Ctrl+Shift
(see keymap.rs); Ctrl+Shift with a menu shortcut's key
(`view::SHORTCUTS`, kept in step with the menus) is Terminal's, and any
other goes to the shell as Ctrl+key, so ⌘R searches history like Ctrl+R. ⌥←/⌥→ send ESC b / ESC f
(a word back/forward in readline, zsh and fish), except to a program
using the kitty keyboard protocol (Claude Code), which gets the
protocol's `CSI 1;3 D` / `C` and reads ESC b as Escape then b.

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

**The keyring's prompts** (`tabula_system::keyring_prompt`, the agent's
`keyring.rs`): gnome-keyring-daemon asks for the login keyring's password
(to unlock it for gh, a browser, …) through a "system prompter" on the
session bus. D-Bus would start GTK's gcr-prompter for that; the agent owns
`org.gnome.keyring.SystemPrompter` (and `PrivatePrompter`) itself from
the start of the session, so the question is a Tabula alert (key icon,
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
`KEYRING_TEST_BUS=unix:path=… cargo test -p tabula-system real_daemon --
--ignored` (makes a keyring, wrong then right password, cancel), or
`tabula-polkit-agent --keyring-test unix:path=…` in a headless session and
a Secret Service client calling `Unlock` and `Prompt`.

## Notifications (apps/notifications)

Tabula's own apps notify through the kit: `tabula_kit::notify::Notifier::
new(cx.proxy(), "Tracker", "tabula-tracker")`, then `show(None,
Note::new("Copied").body(…).action("default", "Show"))` → a `NoteId`;
`show(Some(id), …)` updates it in place (`Note::progress(title, 0.4)`
for a job's bar: transient and silent), `close(id)`. Kinds are
`NoteKind::{Information, Important, Error, Progress}`. The bus is used
on the notifier's own thread; what the user does comes back to
`App::message` as `NoteEvent::Action { id, key, token }` (bring a window
forward with `cx.activate_with(win, token)`) or `NoteEvent::Closed`.
NetPositive's page notifications and Tracker's "Finished copying…" (jobs
of 10 s or more; `TABULA_TRACKER_NOTIFY_AFTER_MS` in debug builds) use it.

`tabula-notifications` serves `org.freedesktop.Notifications` (spec 1.2)
on the session bus, so `notify-send`, browsers, chat and mail clients and
anything else using libnotify reach the user. The real session starts it
(`REAL_SHELL` in tabula-session; an upgrade starts it in a running session
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
  and our own `x-tabula-type`. Each note has a `Kind` after Haiku's
  BNotification: information, important (`im.*` and `call` categories),
  error (critical: stays) and progress. The timeout: -1 is 6 s, 0,
  errors and notes waiting for a reply stay until dismissed. The UI
  answers with `Bus::closed` (reason 1 expired, 2 dismissed, 3 closed by
  the app), `Bus::action` after `Bus::activation_token`, and
  `Bus::replied` (`NotificationReplied`). If another service owns the
  name it waits and retries every 30 s rather than taking it. (zbus's
  `Builder::name` treats "taken" as success; `serve` checks the
  RequestName reply itself.)
- main.rs: one layer surface (Top layer, anchored bottom right) opened
  for the first card and closed with the last, resized to the stack. Up
  to four cards show, grouped by app under a title row with the app's
  icon and name (and a close box for all of a group's), oldest first; the
  rest wait. Important notes wear a blue edge, errors a red one; progress
  shows a `StatusBar`; links in the body open with `open::open`. A card
  shows the picture sent, else the named icon, ours for common theme
  names (`icon_names`), the app's desktop-entry icon, then info/warning.
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
  errors, played on a thread through tabula-preview's `audio::Player`.
- store.rs: the history, `$XDG_STATE_HOME/tabula/notifications/
  history.json` (the newest 200; saved atomically off the UI thread).
  Every note not `transient` goes in, unread unless the History window
  is open; an expired card stays "open" there (its actions still work)
  until the app closes it. After a restart nothing is open, and ids carry
  on after the history's. The History window (`--history`, or Deskbar's
  bell) lists them newest first: a click runs the default action while
  the note is open, else brings the app forward; × forgets one; Clear
  All forgets all but those on screen; opening it marks all read.
- Settings: `tabula_protocol::NotificationSettings` in
  `notifications.json` (config dir): Do Not Disturb (`dnd`, `dnd_until`,
  a daily `schedule` in local minutes), banner `seconds`, and per-app
  `banners` / `sounds` / `history` by app name. The service re-reads the
  file within a second of a change (Preferences ▸ Notifications and
  Deskbar write it). While quiet nothing pops up or sounds but errors.
  It publishes `NotificationState` (`unread`, `quiet`) to
  `$XDG_RUNTIME_DIR/tabula-notifications-$WAYLAND_DISPLAY.json`, which
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
  signal. Flatpak apps reach us through the portal: `tabula-portals.conf`
  leaves Notification to xdg-desktop-portal-gtk, which sends a plain
  `Notify` (no app name: we take the `desktop-entry` hint's) and runs the
  app's action itself when we signal `ActionInvoked`.
- For agents: `org.tabula.Notifications1` at `/org/tabula/Notifications`
  (`List` → JSON, `Invoke`, `Dismiss`, `ClearHistory`, `ShowHistory`,
  `SetDoNotDisturb`), and the same from the command line (cli.rs):
  `tabula-notifications [--bus A] --list [--json] | --history |
  --invoke ID [ACTION] | --dismiss ID | --clear | --dnd on|off|MINUTES`.

Test it on a private bus: `busd -a unix:path=/tmp/tabula-dev/NAME/bus.sock`
(`cargo install busd`; Aurora's dbus-broker-launch needs systemd), then
`tsession run NAME tabula-notifications --bus unix:path=…` and send with
`gdbus call -a unix:path=… -d org.freedesktop.Notifications -o
/org/freedesktop/Notifications -m org.freedesktop.Notifications.Notify --
APP 0 ICON SUMMARY BODY '["default","Open"]' '{}' -1`; `dbus-monitor
--address … "type='signal',interface='org.freedesktop.Notifications'"`
shows the signals (busd refuses `gdbus monitor`'s match rule and drops
some of what it should show). Run it with `XDG_STATE_HOME` (and the
session's `XDG_CONFIG_HOME`) under the test directory so the history and
settings stay out of yours, and `PULSE_SERVER` at a `tabula-fakesound`
socket to hear what it plays. A Deskbar or Preferences in the test
session needs `DBUS_SESSION_BUS_ADDRESS` set to the private bus too, or
its bell's click reaches your own notification service. Unit tests:
`cargo test -p tabula-notifications`.

## The command palette (apps/palette)

⌘Space (`Shortcut::Palette`; Find moved to ⌥⌘Space) or a tap of ⌘ (if
chosen in Preferences ▸ Keyboard) has app_server start
`tabula-palette --toggle`; the running copy (it stays resident, found
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
  commands with their keys (`tabula_kit::commands`, asked of the process
  that owns the front window; disabled items are shown dimmed), Passwords'
  items (`tabula_keychain::Request::Search` while unlocked; choosing one
  is `Request::Show`), files under home by name (indexed by an `ignore`
  walk on start and every five minutes, at most 400,000; matched on their
  own thread for the newest query only), sums (`calc.rs`; Return copies)
  and, last, a web search (Google) in the web browser.
* **Ranking** (`rank.rs`): every word typed must match the title (start,
  word start, anywhere) or the item's keywords, or as initials ("nw");
  then by kind (apps before files) and by how often and how lately each
  thing was chosen (`~/.config/tabula/palette.json`). Nothing typed lists
  what was chosen lately.
* **Keys**: ↑/↓ and Page Up/Down move, Return chooses, the pointer
  hovers and clicks. Actions run on a worker thread.

## NetPositive (apps/browser)

The web browser: Servo 0.7, built in (`engine.rs`), with no system
libraries (no gstreamer: media is ours, see below; no arboard, the kit
has the clipboard; no webgpu, which would bring a second wgpu). Scripts
run on SpiderMonkey with all its JITs (baseline, Ion, wasm), and pages in
content processes of their own (see "Keeping pages apart"). `engine.rs`
also turns on the web features Servo has finished but keeps behind
preferences (WebGL 2, IntersectionObserver, IndexedDB, OffscreenCanvas,
FontFace, container queries…), which sites like Zoom need to load.
One `Servo` per process; each window `embed`s a subsurface and makes a
`WindowRenderingContext` (surfman, EGL) on it in `App::embedded`; each tab
is a `WebView` on its window's context, and only the active one is shown
and painted. Servo draws straight into the subsurface; nothing is copied.

* **Asking before quitting.** Quit, Close Window and the close box go
  through `confirm`: if `what_would_be_lost` finds more than one tab, or
  downloads running that the last window's closing would stop, a
  `Win::Confirm` alert asks first (one per window, and one for Quit).
* **Event loop.** Servo's waker posts `engine::Wake` through the kit
  proxy (one in flight at a time); `App::message` runs
  `servo.spin_event_loop()`. The delegate (`engine::Events`, also the
  clipboard delegate) is called from inside that, while the app is
  borrowed, so it only queues `(WebViewId, Event)`; `pump` acts on them
  afterwards, and paints the active view of each window that got a new
  frame (`paint` + `present`).
* **Never blocking.** surfman leaves EGL's swap interval at 1, and Mesa
  then waits for a frame callback on every swap: a hidden window would
  hang the UI. `Surface::new` sets it to 0; Servo's refresh driver paces
  frames. When the last surface goes (`embed_lost` on a reconnect),
  `Surface::terminate` calls `eglTerminate`, so a new connection at the
  same `wl_display` address can't get Mesa's cached display for the dead
  one. Views are dropped before their context (the last view's `Painter`
  deinitialises WebRender with the context current); after a reconnect
  each tab reloads its URL on the new surface (history and form state are
  lost; `save_state` keeps windows, tabs and the active one across a
  binary upgrade).
* **Input** (`input.rs`): the content rect is an egui widget
  (`Sense::click_and_drag`, focus lock for Tab/arrows/Escape); window
  events over it become Servo `InputEvent`s in device pixels: pointer
  (a press in the page keeps the pointer until release), wheel (egui's
  delta negated: DOM deltas scroll the view), keys (named keys as key
  events, text as key down/up per character), IME, and Copy/Cut/Paste
  (`Event::Paste` stores the text the clipboard delegate answers the
  page's paste with). The view's focus is the page being the window's
  keyboard target (`page_focus`), set *before* a click is forwarded and
  kept while popups have the keyboard: a focus change after the click
  would blur the element and close a `<select>`'s list.
* **Page controls** (`EmbedderControl`): context menus and `<select>`
  lists are kit popups (`MenuAnchor::At`, and `Over` with the chosen
  option over the field, as a BeOS menu field). Their positions come in
  CSS pixels, scaled by the page and pinch zoom. `alert` / `confirm` /
  `prompt` are BAlerts over the window, HTTP logins an alert with Name and
  Password, `<input type=file>` a kit `FilePanel`; Servo's own permission
  requests (location, notifications) are denied for now (cameras and
  microphones are ours: see "Cameras, microphones and calls" below).
* **Colour schemes** (`scheme.rs`): pages that support dark go dark
  with the desktop. Stylo takes system colours (`CanvasText`) from the
  preference even for pages that don't choose a colour scheme, and Servo
  paints the canvas with a single clear colour, never the page's
  `Canvas`, so a zero-specificity style gives other pages `color-scheme:
  light` and dark ones a Canvas background.
* **Dark pages** (`dark.rs`; View ▸ Dark Pages: Automatic, the default,
  follows the desktop; Always; Never; `dark_pages` in `browser.json`).
  Done as Chrome's "Auto Dark Mode for Web Contents" does it: by what
  each thing a page draws is, in our WebRender (`vendor/webrender`,
  `src/dark.rs`; see vendor/README.md), never to the finished frame.
  The scene builder turns CIELAB lightness around (L to 110 − L, hue and
  chroma kept) for light backgrounds (rectangles, gradient stops; brighter
  than 128 on Chrome's 0–255 scale), light or near-black borders, and
  dark text and lines (below 150); colours already right for a dark page
  stay, so a page that is dark by itself is left nearly alone with no
  detection. Pictures, video and canvases are untouched, except small
  ones (160 device pixels or less each way) that Chrome's image
  classifier calls icons (few colours, not light-on-transparent, not
  colourful): those are uploaded with their lightness turned around, so
  a black SVG glyph shows white. The clear colours go dark too (blank
  tabs, pages without a background). `dark::set` flips a process-wide
  switch; Servo's painter sends WebRender a transaction at its next render,
  the scene builder rebuilds every document whose scene predates the
  change, and icons are re-uploaded the other way round.
* **Resources** (`resources/`): Servo's defaults (MPL-2.0, see NOTICE)
  with our own error, certificate and crash pages, which answer in haiku
  as NetPositive did (new verses). `engine.rs` registers the reader.
* The location field takes addresses, paths and searches
  (`location::parse`, unit-tested; searches go to DuckDuckGo's HTML
  page). A new tab is blank with the field focused; its first load makes
  a fresh view, so the blank page isn't in its history. Links opened from
  the context menu go to a background tab; `window.open` and
  `target=_blank` to a new foreground tab. Settings (home page,
  bookmarks, the last 1000 pages for the Go menu) are in `browser.json`;
  cookies, local storage and the HTTP cache in
  `$XDG_DATA_HOME/tabula/browser` (Servo's `config_dir`).
* **Notifications** (`notify.rs`): Servo's Notification API is on. A
  site's first `requestPermission` puts a bar over its tab (Allow / Don’t
  Allow); the answer is kept by origin in `browser.json`
  (`notifications`) and given straight away after. While the bar is up
  the page's script waits on it, so the hang guard waives that tab.
  Allowed notifications go to the desktop's notification service from a
  worker thread (app "NetPositive", our icon, the site under the body,
  tags replace); a click brings the tab forward. Servo 0.7 only shows a
  notification after loading its pictures, never one without: a page
  script gives those a one-pixel icon. (Servo has no way yet to fire the
  page's `onclick`.) Test with a private bus (see Notifications) and a
  page on `http://127.0.0.1`, NetPositive started with
  `DBUS_SESSION_BUS_ADDRESS` set to it; page processes then stay out of
  systemd, since `guard::hand_off` only asks a systemd that is on the bus.
* Building it turns on vello_cpu's `multithreading` feature for every app
  built alongside (Servo's canvas wants it), which broke egui's glyph
  rasterizer: hence `vendor/epaint` (see `vendor/README.md`).

## Other toolkits' programs (app_server toolkits.rs, state.rs; tabula_theme::toolkit)

Programs not built on the kit take Tabula's look as far as their toolkit
allows. `toolkits.rs` publishes the palette where GTK 3, GTK 4,
libadwaita and Chrome read it (GSettings through dconf, the Tabula and
Tabula-dark GTK themes, GTK 4's user stylesheet, BeOS's close-left,
zoom-right title buttons); see its module comment.

Window chrome: app_server offers server-side decorations two ways.
xdg-decoration (`zxdg_decoration_manager_v1`) is what Chrome, Qt and most
others ask through. GTK 3 and GTK 4 don't speak it; they speak KDE's
`org_kde_kwin_server_decoration`, whose default mode app_server
announces as Server, so a GTK window with no headerbar draws no
decorations of its own and gets Tabula's tab and frame
(`KdeDecorationHandler` in state.rs, `kde_server_side`, read in
`shell/xdg.rs` with the xdg-decoration mode). A window that asks for
Client (a GTK headerbar, so every libadwaita app) keeps its own,
coloured by the theme. Test with a throwaway GTK script in a headless
session (`tsession run NAME GDK_BACKEND=wayland python3 test.py`, the
private bus as DBUS_SESSION_BUS_ADDRESS).

## Google Chrome (apps/chrome)

`tabula-chrome` is Google Chrome in Tabula's applications (`OUR_APPS`).
With Chrome installed it execs Chrome's own launcher
(`~/.local/share/tabula/chrome/current/google-chrome`) with its arguments,
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
`google-chrome.desktop` that starts it through `tabula-chrome` (NoDisplay,
as Tabula lists it itself; rewritten when it isn't ours). Nothing needs
root. Once a day (`.checked`), a start also runs `tabula-chrome
--tabula-update` in the background: a newer version is put in beside the
running one and becomes `current` for the next start (the one it
replaced is kept, as Chrome reads its files as it runs; older ones go;
`.updating` keeps it to one at a time). Test it with a fixture HOME (`tsession run NAME HOME=… XDG_DATA_HOME=…
target/debug/tabula-chrome`), never Ben's profile.

## Log Viewer (apps/logs)

`tabula-logs` shows every Tabula program's log in one place: what they
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
  GLib's `-WARNING **` and panics recognised, and Tabula's own stderr
  lines Info unless they say otherwise (the session's stream files all
  of stderr as warnings). Continuation lines (a pretty debug print, a
  backtrace) join the line before from the same process. `Query`: time
  (last 10 minutes, hour, today, since startup, all), minimum level, one
  program or Tabula's or all, and text (in the message or the program).
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
directly: `tabula-logs --print [--since 10m|2h|1d|today|boot|all] [--level
debug|info|warning|error] [--program NAME | --all-programs] [--find TEXT]
[--json] [--follow]` (JSON lines: time in UTC, program, pid, level,
message). Tests: `cargo test -p tabula-logs` (synthetic journal files
in both layouts); `-- --include-ignored` also reads this machine's
journal and highlights every line in it.

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

## Passwords (apps/passwords, crates/onepassword)

`crates/onepassword` (package `tabula-onepassword`) is a Rust 1Password SDK
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
`TABULA_1PASSWORD_DATA` overrides), read-only and nothing else from it.
Passwords' setup offers them as a choice and keeps the UUID, which the
SDK integration takes as the account's name.

Tests never touch the real app. `onepassword-fake [--account NAME]
[--socket NAME] [--data FILE] [--accounts DIR]` (DIR gets a
`1password.sqlite` listing it) serves an in-memory account (two vaults of
sample items, item versions, OTP codes) over the same protocol;
`tests/desktop.rs` drives it. Apps take `TABULA_1PASSWORD_SOCKET` to use it.

Passwords is the app: sources (vaults, categories, Archive), a two-line
item list and the selected item's fields (hover for Copy/Show; one-time
codes count down), an editor with templates per category, a Password
Generator window, and File/Edit/Item/View menus (⌘C username, ⇧⌘C password,
⇧⌘O one-time code). Secrets go to the clipboard with `copy_secret` and
leave after `clear_seconds`. It opens locked and locks after
`lock_minutes` idle, and when the screen locks if `lock_with_screen`
(`lock.rs`): Touch ID through fprintd, claimed only
while the locked window has the keyboard, or the account password through
PAM. Those, and `auto_sync`, are Preferences ▸ Security's
(`Settings::security.passwords`, followed live through
`tabula_kit::settings::watch`; the screen locking comes from
`settings::watch_power`, `PowerStatus::locked`; View ▸ Lock After writes
the same setting with `settings::update`). Its own settings (accounts,
the generator, the window size) live in `passwords.json`.

**Its own store** (`vault.rs`). Every vault and item lives on this
computer, so the app, NetPositive and the palette never wait on
1Password: `passwords.vault` in the config directory, "TABV", a format
byte, a 24-byte nonce and the JSON sealed with XChaCha20-Poly1305 (header
as associated data), written whole and atomically (0600) after each
change. The key is 32 random bytes in the login keyring
(`tabula_system::secrets`, a Secret Service client; PAM unlocks the
keyring at login, so Touch ID still opens Passwords). A store whose key is
missing is never overwritten with a new one. `TABULA_PASSWORDS_KEYFILE`
keeps the key in a file instead; tsession sets it for every session, so
test apps never touch the real keyring. The worker (`store.rs`) owns the
store: it opens it at the unlock, lets it go at the lock (after a running
sync), and answers the UI and NetPositive from it. Items keep 1Password's
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

`cargo test -p tabula-passwords` covers the store (sealing, wrong keys,
conflicts, races, accounts, reset) and syncing against the in-process fake
(both ways, conflicts, nothing to do, 1Password down, that a plan
changes nothing and names every change, archived items and an
unlistable vault, and several accounts in one review). The Secret Service
client has an ignored test for a private keyring:
`tools/dev/private-keyring start` (busd plus its own gnome-keyring; needs
`cargo install busd --root /tmp/tabula-dev/busd`), then
`DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/tabula-dev/keyring/bus cargo test
-p tabula-system secrets -- --ignored`; it refuses any other bus.

```sh
target/debug/onepassword-fake --socket tabula-fake &
tools/dev/tsession start pw 2400x1600 --light
tools/dev/tsession run pw TABULA_1PASSWORD_SOCKET=tabula-fake TABULA_NO_FINGERPRINT=1 \
    TABULA_PAM_CONFDIR=$dir TABULA_PAM_SERVICE=exec target/debug/tabula-passwords
```
(`run` doesn't pass `TABULA_PAM_*` on by itself; give them as above, with
`$dir` set up as for the lock screen.) Account: `Tabula Test` (choose
Sync in the setup); password: whatever `check.sh` accepts. Pass
`--data FILE` to the fake to see what reached "1Password".

### Find in NetPositive

Servo 0.7 has no find API, so `apps/browser/src/find.rs` builds a script
from the Find window's settings (case, whole words, wrap, backwards) and
evaluates it in the top frame: it walks the shown text nodes (skipping
scripts, styles and anything without boxes), searches from the current
selection, selects the match and scrolls it to the middle. The selection
is the cursor, so Find Again (⌘G) continues and a click moves the start.
Not found puts a note in the status line.

### Scroll bars in NetPositive

Servo 0.7 draws no scroll bars and doesn't tell the embedder where a page
is scrolled, so `apps/browser/src/scroll.rs` adds a script to the top
frame that reports `[scrollX, scrollY, width, height, innerWidth,
innerHeight]` whenever one changes (on scroll, resize and a
ResizeObserver, at most once a frame). NetPositive draws a kit
`ScrollBar` beside the page (always the vertical one, the horizontal one
when the page is wider than the window); moving one runs `scrollTo`, and
for a moment the bar trusts its own position over the page's reports,
which lag a frame or two.

### Tab icons in NetPositive

Servo decodes a page's favicon and says when it changes;
`apps/browser/src/favicon.rs` scales it to a 32 px square (an area
average going down, whole pixels going up, aspect kept) and the tab strip
draws it at 16 points. A page that loads without one gets the generic
`document` icon instead of the last page's.

### Dark pages in NetPositive

NetPositive tells Servo the desktop's theme (`notify_theme_change`, also
when it changes), so `prefers-color-scheme: dark` works. Servo 0.7 takes
system colours from that preference even for pages that chose no colour
scheme, and doesn't paint a dark page's canvas, so
`apps/browser/src/scheme.rs` adds a zero-specificity style to every
document in dark mode: `color-scheme: light` for pages that don't support
dark, and a Canvas background (plus Highlight selection) for those that
do, judged by `<meta name=color-scheme>` and the root's computed
`color-scheme`.

### Cameras, microphones and calls in NetPositive

Servo 0.7's WebRTC is a stub, so `apps/browser/src/rtc/` does it. A user
script (`shim.js`) gives top-level secure pages `navigator.mediaDevices`,
`MediaStream(Track)`, `RTCPeerConnection`, transceivers, senders,
receivers, `RTCDataChannel` and `getStats()`. It talks to NetPositive
through `console.log` with a per-process marker; answers and events come
back through a window function only the token names (a reply carries
`id`, an event `ev`).

* **Devices** (`capture.rs`): cameras through Video4Linux (pure Rust;
  NV12, YUYV and MJPEG made I420), microphones through the sound server.
  The page asks first: a bar under the toolbar, remembered per site until
  NetPositive quits. `TABULA_BROWSER_FAKE_CAMERA=1` makes a test card the
  only camera (real ones aren't even listed), for tests; with
  `PULSE_SERVER` at tabula-fakesound the microphone is fake too.
* **The hub** (`hub.rs`): sources (a device, the far end of a call)
  publish frames; tracks are a page's handles on a source; streams group
  tracks. `<video>`/`<audio>` playing a stream (`srcObject`) read it from
  NetPositive over a socket (`live.rs`) into our media player in the
  page's process.
* **Calls** (`peer.rs`, `codec.rs`): webrtc-rs 0.21 in process, on a
  2-thread tokio runtime. Each connection runs the page's calls in order
  on a task of its own. Sending: a *slot* per transceiver, our own track
  fed from a hub track by an encoder thread (H.264 Constrained Baseline
  via openh264, Opus 48 kHz); `replaceTrack` just moves which hub track a
  slot reads. Receiving: each transceiver is a hub source; H.264 is
  depacketized into access units and decoded on a thread of its own,
  which waits for a key picture and asks for one (PLI) after a gap. An
  interceptor (`KeyRequests`) passes the far end's PLI/FIR on to us,
  since webrtc-rs keeps inbound RTCP to its interceptors by default.
  Kinds and m-line order come from the descriptions (a transceiver
  doesn't know its kind). `getStats()` is webrtc-rs's report plus our
  picture counts.
* **Testing**: two `RTCPeerConnection`s in one page, the fake camera and
  `tabula-fakesound` (its log shows the far end's sound arriving); see
  the "a call, end to end" journal entry.

### Audio and video in NetPositive

Servo plays media through a servo-media backend; built without gstreamer
it would play nothing, so `apps/browser/src/media.rs` installs ours before
Servo starts (the first backend to arrive wins). It plays with Viewer's
decoders: `tabula_preview::video::Video` (H.264 / AV1 in mp4 / WebM /
Matroska) and `audio` (symphonia, Opus), which read from an
`input::Input`, a file or anything `Read + Seek`. Servo pushes the fetched
bytes into a shared buffer; readers block for bytes not yet there, so
playback starts early and seeking ahead waits. A driver thread per player
keeps the clock (the sound's, or its own without a sound server), hands
Servo each picture as BGRA, and reports state, position and the end.
Web Audio still goes to servo-media's dummy backend (streams and calls
are ours, below).
In multiprocess mode the player runs in the content process, which
installs the backend too (`guard::content_process`).

Memory follows what's playing, not how many videos a page has (slack.com
starts twenty-eight, many 2920×2000):

* **Downloads** keep their first MiB in memory and the rest in a
  nameless file on disk (`tabula_xdg::scratch_file`: O_TMPFILE under
  `~/.cache/tabula/scratch`, not /tmp, which is memory); readers `pread`
  it without holding up the bytes coming in.
* **Decoders are budgeted per page** (`decode_pixels`: the page's memory
  limit / 128 in pixels, at most 18 M). A player asks `start_decoding`
  before making one; over budget, the decoders that started longest ago
  (and at least `TENURE`, 1 s, ago; never audible ones) are asked to let
  go (`Cmd::Hold`) and the player tries again once they've gone (a let-go
  decoder counts until its thread has finished: `Video::close` hands it
  back, then `malloc_trim`). Only playing or seeking evicts: a first
  picture, or a player held while playing, waits patiently for room
  others leave. A held player keeps its picture on show. A player paused for
  `IDLE_RELEASE` (2 s) with its picture up lets its decoder go too.
* **Muted videos out of sight pause** (the page script, with an
  IntersectionObserver, as other browsers do; also when autoplay starts
  one already out of sight) and play again when scrolled back, so their
  decoders go and the visible ones get the room. A page's own `pause()`
  or `play()` overrides that.
* **Queued frames are compact**: YUV planes (`Pic::H264` / `Pic::Av1`),
  converted to BGRA (`Options::bgra`, what Servo takes, no copy) only at
  the front of the queue, and read-ahead is bounded in bytes
  (`QUEUE_BYTES`, 24 MiB: two to six frames).

Each video element still holds one full-size picture in Servo (it takes
the natural size from the frame, so a smaller one would resize the
element). slack.com settles at about 840 MB (and loads within 512 MB);
forty 2920×2000 videos at about 500 MB.

A page script works around Servo 0.7: it fires `timeupdate` only once
something has read an element's `textTracks` (so the script reads it as
elements arrive), and after reaching the end it stays at
HAVE_CURRENT_DATA and never plays again (so `play()` on an element that
ended reloads it first).

### Keeping pages apart in NetPositive

Servo runs in multiprocess mode: each site's pages (an event loop) get a
content process, which is this binary again with `--content-process
TOKEN` (`main` checks first). `guard.rs` then keeps each one boxed in:

* **A systemd unit per process.** The process Servo starts asks the
  user's systemd (session bus) to run the same command as a transient
  service, `tabula-browser-<ui pid>-<pid>.service`, waits for it to be
  active and exits; the service's process connects to Servo in its place.
  The unit has `MemoryMax` (`guard::page_memory`: a quarter of RAM,
  1.5–4 GiB; no swap, so a runaway page is OOM-killed instead of
  grinding), `TasksMax` and a `CPUWeight` NetPositive changes later
  (`SetUnitProperties`): 100 for the tab you're looking at, 20 hidden,
  1 stuck. The user manager owns its cgroups, so this needs no
  privileges, and weights go both ways (nice can only go down without
  `RLIMIT_NICE`). Without systemd the page stays where Servo started it,
  niced to 5 with an `RLIMIT_DATA`, and the watch kills it over its RSS
  limit. A page in a unit isn't NetPositive's child, so it watches
  NetPositive's pid (`TABULA_BROWSER_UI_PID`, a pidfd) and exits with it;
  one left in process has `PR_SET_PDEATHSIG`.
  `TABULA_BROWSER_PAGE_MEMORY` (MiB) sets the limit for tests;
  `TABULA_BROWSER_SINGLE_PROCESS=1` runs everything in one process.
* **Which tab is which process.** Every two seconds each tab is asked, by
  `evaluate_javascript`, for its process id: `canPlayType` of
  `video/x-tabula-pid; key=…; bit=N` is answered by our media backend in
  the content process, bit by bit (`guard::pid_bit`; the key is a random
  one in the environment, so pages can't ask). A tab that hasn't
  answered in ten seconds is stuck: a bar offers Wait or Stop Page, which
  kills the process.
* **Ends.** The `page-guard` thread lists our units each second, learns
  their main pids, and when one fails reads `Result` (`oom-kill` says
  "used too much memory" in the tab's bar), then resets it. A NetPositive
  that crashed leaves failed units, collected on the next start.
* Servo 0.7 never noticed a content process dying (its pages stayed on
  the dead event loop and reloads went nowhere), and by default exited
  the browser when any page panicked: `hard_fail` is off, and
  `vendor/servo-constellation` treats a process whose lifeline breaks as
  its event loop panicking, so its tabs get the crash page and reload in
  a new process.

### Colours in NetPositive

`<input type=color>` arrives as an `EmbedderControl::ColorPicker`;
NetPositive opens a "Choose a Colour" sheet with a kit `ColorControl`.
OK selects and submits the colour; Servo 0.7 then sets the value but
fires no events, so a script waits for the value to land and dispatches
`input` and `change` on the focused colour input. Cancel submits the
colour the page had.

### Downloads in NetPositive

Servo 0.7 has no download API, so `apps/browser/src/downloads.rs` finds
downloads itself and fetches them with ureq on a worker thread (the page's
cookies, user agent and referrer go along). A user script reports three
things over the console channel: Servo's "Unknown content type" page (the
tab goes back and the URL is downloaded), clicks on `a[download]` links
(also detached anchors clicked from script, and `a.download`, which Servo
doesn't reflect, is polyfilled), and `blob:` contents, read in the page and
sent in base64 chunks since only the page can read them. `data:` URLs are
decoded in process. The context menu adds Download Linked File and
Download Image. Files go to the XDG download folder as `name.part` and are
renamed when complete (`name (2).ext` if taken); the name comes from
Content-Disposition, the `download` attribute or the URL. The Downloads
window (Window menu) lists them newest first with live progress and rate,
Stop, Retry (HTTP only), Open and Show (Tracker on the folder).

### Logins in NetPositive (crates/keychain)

While Passwords runs it serves other Tabula apps on
`$XDG_RUNTIME_DIR/tabula-keychain-$WAYLAND_DISPLAY.sock`
(`TABULA_KEYCHAIN_SOCKET` overrides): one JSON request, one JSON reply per
connection, and only for our own uid running a `tabula-*` program
(SO_PEERCRED + `/proc/PID/exe`). `tabula_keychain::call` blocks, so call
it from a thread. Requests: `Logins {url}` (title and username of each
login whose website fits the page, honouring 1Password's autofill
setting; `Locked` if locked), `Fill {url, vault, id}`, `Check {url,
username, password}` (`Known`, `New` or `Changed`), `Save {…, replace}`,
and `Unlock`. Fill, Save and Unlock wait while Passwords is locked: it
comes forward and answers after the unlock. `tabula_keychain::matches`
is the site rule, as 1Password's: the same registrable domain (the `psl`
crate's public suffix list, so mail.google.com's login fills
accounts.google.com but alice.github.io's never fills bob.github.io), the
same host where there's none (localhost, addresses), and never an https
login into a plain-http page (localhost aside). Every login that fits is
listed, nearest first (`tabula_keychain::closeness`: the page's own host,
then its parent domains from the nearest, then the site's other hosts),
then by title; an item that can't be read is logged, not hidden silently.

NetPositive (`apps/browser/src/logins.rs`): the toolbar's key button asks
for the page's logins (starting Passwords if needed, then waiting for the
unlock) and fills the chosen one into the top frame with
`evaluate_javascript`, after checking the page is still at that origin; a
one-time code goes on the clipboard with `copy_secret`. The page's
context menu in a text or password field ends with a Passwords submenu:
each of the page's logins with its Username and Password, which fill just
that field (`logins::field_script`, the focused element, origin checked).
The page's logins are fetched when it finishes loading (`Purpose::Prefetch`)
and kept per host for `KNOWN_FOR` (10 min), so the menu opens with them at
once; otherwise it waits up to `FIELD_MENU_WAIT` (400 ms), then opens
without them (Open Passwords).

Finding the fields (`logins::fill_script`): only fields a person could
use count (laid out at least 4 px, on screen, visible, not aria-hidden,
disabled or read-only), so decoys like Google's hidden password field are
skipped. With no password field on the page (two-step sign-ins), the
username goes into the focused text field, else one marked
`autocomplete=username`, else one whose name, id, aria-label or
autocomplete looks like a user, mail, login or phone field, else an email
field, else the only text field. **Learnt fields**: filling a field from
the context menu records its selector (id, name, or an nth-of-type path)
for that page (origin and path, no query) in
`netpositive-fields.json` in the config dir (`logins::Learnt`), and the
key button tries the learnt fields first next time. A user script
(Servo's `UserContentManager`) watches the top frame for a form sent with
a password and reports it through `console.log` with a per-process random
token held in its closure (`show_console_message` drops everything
else), so an iframe can't make up logins for the page around it. Unless
Passwords already knows it, a bar offers Save (or Update).

Testing: serve a form on localhost (the fake's "Tabula Test Site" login is
for `http://localhost`), and start NetPositive with the same variables as
Passwords so the copy it starts uses the fake.

**Passkeys** (`apps/browser/src/passkeys.rs`, `apps/passwords/src/passkey.rs`).
A second user script gives top-level secure pages WebAuthn: its own
`navigator.credentials` (`create`/`get` with `publicKey`; `store` and
conditional mediation aren't offered) and `PublicKeyCredential` with the
response classes, `toJSON` and the `parse…FromJSON` helpers. Requests reach
NetPositive like captured logins (marker + token); answers go back by
`evaluate_javascript` calling a function named with the token. NetPositive
checks the relying party (the page's host, or a registrable domain above
it, via `servo::is_reg_domain`), asks in a bar (Save Passkey; Sign In with
a pop-up when there are several), builds the client data and sends its
hash. Passwords checks the relying party against the page again, makes
P-256 keys from the system's random source, keeps each in a login item's
"Passkey" section (credential ID, relying party, user handle and name, the
key concealed: replacing the same account's passkey, joining the login with
that username, or a new login), and signs `authenticatorData ‖ hash`.
Attestation is "none" with a zero AAGUID; flags are UP, UV (Passwords is
unlocked), BE and BS; the counter stays 0. 1Password syncs the section but
its own apps don't treat it as a passkey. Only ES256; no iframes.
