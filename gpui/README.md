# Olo's GPUI

The UI framework Olo's kit is moving to (from egui; see "The kit" in
[docs/TODO.md](../docs/TODO.md)). It began as GPUI 0.2.2 from crates.io,
Zed Industries' framework, under the Apache License 2.0
([LICENSE-APACHE](LICENSE-APACHE)), and is Olo's own now: changed as Olo
needs, never synced with upstream.

- `gpui`: the framework. `macros`, `collections`, `refineable`,
  `derive_refineable`, `sum_tree`, `semantic_version`: its helpers, as
  published. `util`: Zed's `util` cut down to what GPUI uses.
- `blade-graphics`: the GPU layer GPUI draws with (Vulkan here), from
  blade-graphics 0.7.1 by Dzmitry Malyshau and the Blade contributors,
  under the MIT licence (https://github.com/kvark/blade). It replaces
  the crates.io release for the whole workspace (`[patch.crates-io]`).

What Olo changed, so far:

- Wayland only: the X11 platform and its dependencies are gone, and a
  missing `WAYLAND_DISPLAY` means headless. (The macOS and Windows
  platforms are still in the tree but not built; they'll go.)
- Nothing fetched from the network: images load from files and assets,
  not URLs (no `http_client`); opening a URI or revealing a file goes
  through the portal only (no `xdg-open`).
- A window draws only when something changed. Upstream drew and
  committed every frame for as long as the window was open (and
  presented the same scene again for a second after input); Olo's asks
  for a frame callback only while it animates, so an idle window costs
  nothing and lets app_server rest. See `FrameLoop` in
  `gpui/src/platform/linux/wayland/window.rs`. (GPUI runs the main
  thread's tasks in calloop idle callbacks, and calloop doesn't wake
  for an idle queued from one, so queuing a frame wakes the loop: else
  a task's `refresh` waited for the next input.)
- Paths drawn with exact coverage (`gpui/src/outline.rs`): a path built
  with `PathBuilder` keeps its outline, and a small one in a plain
  colour (a check mark, a menu's triangle) is filled or stroked by
  vello_cpu into a coverage mask, cached in the atlas by its outline on
  the device grid and drawn as a sprite; larger ones are filled on the
  GPU as before. Upstream's 4× multisampling gave an edge pixel one of
  five shades; this gives it its true coverage, as egui and Skia do,
  and drawing the gallery's controls costs a third less (683 µs a frame
  against 1076 at 2×) in 20 MB less memory. `OLO_EXACT_PATHS=0` turns
  it off to compare.
- Scrolling as the egui kit's: a wheel's notch scrolls 48 points, eased
  in over the frames to come (90% in a tenth of a second, a step per
  frame at the display's rate, ending once less than half a device
  pixel is left, or at an edge), where upstream jumped three lines at
  once; fingers on a touchpad scroll their own distance (upstream:
  three times it), from a first move (`TouchPhase::Started`) to the
  stop (`Ended`), and app_server's glide after them the same way.
- Line segments drawn on the GPU (`Window::paint_segment`): any angle
  and width, butt or round ends, each pixel's coverage from its
  distance to the line, one instance a segment (a graph's hundred
  points: one draw call), where a path would be tessellated each frame.
  A translucent polyline darkens a little where its segments meet.
- The clipboard offers each form of what is copied under its own MIME
  types: text under the text ones, an image under its format's, and
  `ClipboardEntry::Data` (`ClipboardItem::new_data`, `from_entries`)
  under its own (files as `text/uri-list`), where upstream offered text
  only.
- Ring slices drawn on the GPU (`Window::paint_sector`): between two
  radii and two angles (clockwise from 12 o'clock), each pixel's
  coverage from its distance to the edges, parallel edges box-filtered
  together so one thinner than a pixel covers by its width; a disc from
  radius 0 round a whole turn, a circle's outline a ring a line wide.
  One instance on the quad of its bounds, where a path was tessellated
  and its mask rasterised: DiskUsage's 68-slice chart paints in ~40 µs
  (was 3.2 ms). `Window::paint_layer` gives everything inside one place
  in the scene's order, so hundreds of segments or sectors cost one
  bounds-tree insert (Graph's series, the chart's slices and lines).
- Layer surfaces (wlr-layer-shell): `WindowKind::LayerShell` with
  `WindowOptions::layer_shell` (layer, anchored edges, exclusive zone,
  margins, keyboard, namespace, output by name), for Deskbar, the
  desktop, notifications and overlays; popups open over them too
  (Deskbar's menus).
- `Window::draw_now` draws a window even out of sight (for a layout
  review).
- Drags out of a window (`Window::start_drag(OutgoingDrag)`): a Wayland
  data source offering each type with its data, the actions allowed and
  a picture under the pointer (a surface from shared memory, at the
  window's whole scale); what it was dropped as comes back on a
  channel. An app's own drag isn't turned into a file drop on it.
  `TextSystem::line_mask` draws a line of text on the CPU (a drag's
  picture's name).
- Tips (`WindowKind::Tip`): an xdg_popup with an empty input region and
  no grab, below its anchor (else above, slid on screen), for tooltips
  that reach past a small window's edges.
- `App::close_window` closes a window even from inside an update of it
  (upstream's `handle.update(.., remove_window)` silently does nothing
  there: a window's own Cancel button or key).
- `Window::surface_id`: the wl_surface's protocol id, which app_server
  names windows by (its keyboard hints).
- `Window::push_paint_record` and `paint_records`: what the app notes
  as it paints (olo-ui's agent nodes), kept with the frame and replayed
  with a cached view's paint, so a view drawn from its last frame still
  tells agents what it shows.
- `Window::activate_window_with_token`: raises a window with an
  xdg-activation token another program handed over (a notification
  clicked), which lets it take the focus; upstream only asks for one of
  its own, which compositors refuse.
- No desktop portal watched (the colour scheme, the cursor's theme and
  size): olo-ui follows app_server, which draws the pointer, and the
  portal's settings over D-Bus kept a `blocking` pool thread waking in
  every app. Lines wrap only at white space (and between Chinese and
  Japanese characters): a curly apostrophe, quotes, brackets and a
  path's slashes stay with their word, as egui wraps.
- The fractional scale is the window's scale: on a wl_compositor
  before version 6 (app_server's), entering a display set its integer
  scale over the fractional one (2 over 1.5), and the compositor shrank
  the 2× buffers to fit, every line smeared over two pixels.
- `Window::with_element_opacity` is public (a disabled control,
  faded), and popups (`WindowKind::Popup`) take an `offset` from their
  anchor (a menu field's menu, its current item over the field).
- GPU memory sized for many small windows on unified memory: the
  allocator's chunks start at 256 KB and stop at 2 MB, anything of 2 MB
  or more is its own allocation (blade's prototyping defaults grew
  chunks to 128 MB: hello_world held 96 MB, mostly empty), and the
  textures paths are drawn through (window-sized, one of them 4×
  multisampled) are made only when a window first draws a path.
- The GPU device is made with the first window: an app with none (the
  polkit agent waiting for a request) holds no driver threads, and
  half the memory (13 MB against 24).
- Windows share the device's pipelines and its atlas (`BladeContext`'s
  `PipelineCache`, by surface format, alpha and paths' sample count, and
  one `BladeAtlas`): upstream made both for every window, so each menu
  and each ⌘Space compiled the shaders and built ten pipelines (28 ms)
  and rasterized and uploaded every glyph and icon again for its first
  frame. A tile's key holds all it depends on (size, scale, the
  display's text rendering). A window's surface is committed (and the
  connection flushed) before its renderer is made, so the compositor's
  first configure comes back meanwhile; nothing is drawn before that
  configure is acknowledged (a scale sent to a new surface asked for a
  frame early: a protocol error that ended the program). The command
  palette opens in 28 ms (egui's: 47).
- Background threads start when there's work and none is free, and
  leave after 10 s without any (upstream kept one per core).
- Olo's text (`gpui/src/platform/linux/text_system.rs`), the same as
  the egui kit's: Olo's own faces (data/fonts) before the system's,
  shaped by harfrust, outlines hinted by skrifa's autohinter (vertical
  only, "light") and filled by vello_cpu; variable faces give every
  weight on their axis; a character a face lacks comes from the kit's
  chain (Olo's symbol faces, the other of Noto Sans and Noto Sans Mono,
  DejaVu Sans, CJK, colour emoji), on the line's one baseline. GPUI's
  cosmic-text and font-kit are gone (swash has no autohinter, so Noto
  drew unhinted, and cosmic-text 0.14 had no variable weights). The
  system's fonts load only when something needs them. Small text is
  darkened (stem darkening, `darkening`) by growing its outline a
  little all round, FreeType's emboldening (`embolden`: each point
  moved out along its corner's bisector), then filled: a stroke round
  the outline did the same at three times the cost (40 µs a glyph
  against 13), and two layers' blending made its fringes a shade
  heavier (total ink within 1%, the same look at every scale).
- Text drawn for the display a window is on (`TextRendering`, chosen by
  `olo_protocol::panels::text_mode` from the display's density, colour
  stripes and rotation): grey-scale unhinted at Retina density (as
  macOS), LCD subpixel coverage on low-density striped LCDs (FreeType's
  LCD filter, blended per channel with dual-source blending, which
  blade-graphics gained), hinted grey-scale in between. A window drawn
  again when it moves to a display that wants another.
  `set_display_text_rendering` takes the person's choice from
  Preferences ▸ Display; `OLO_TEXT_RENDERING` forces one for tests.
