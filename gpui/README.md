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
- `blade-util`: its buffer belt (per-frame GPU buffers), from blade-util
  0.3.0, same authors and licence: chunks are made a power of two in
  size, so a frame that grows reuses them rather than making a new one
  every frame (none is ever freed).

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
  `Window::paint_segments` paints many of one colour in one operation
  (a polyline's pieces): StringTheme's thousands at 4.5% of a core,
  against 6.0 with a call each.
- Pictures drawn as textured triangles (`Window::paint_image_triangles`,
  the `ImageTriangle` primitive): each corner a point on screen and one
  in the picture, sampled from the sprite atlas and tinted, for a
  picture bent or magnified (the screen saver's Flip and Lens).
- The clipboard offers each form of what is copied under its own MIME
  types: text under the text ones, an image under its format's, and
  `ClipboardEntry::Data` (`ClipboardItem::new_data`, `from_entries`)
  under its own (files as `text/uri-list`), where upstream offered text
  only. `App::clipboard_types` says what's there to paste (to enable
  Paste); `App::paste_as(wanted)` reads it as the first of the types
  wanted it's offered as (`image/` standing for any picture), on a
  worker, giving up on a program that takes over five seconds to write
  it (upstream read the clipboard on the UI thread until the other
  program finished). When the clipboard changes (another program
  copied, or this one), the app's windows are drawn again, so their
  menus' Paste follows it (`Platform::on_clipboard_changed`).
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
- A window's first frame is drawn once, for the display it opens on.
  Upstream drew it at scale 1 and with the default text rendering
  (before the compositor said otherwise), then again: 6–8 ms more to
  open a window, glyphs drawn for a scale it's not at. The client reads
  the displays when it connects (one roundtrip); a new surface asks its
  scale on an event queue of its own while its renderer is made
  (`ScaleProbe`, 60–180 µs, a fractional-scale object it then drops for
  the window's own) and draws its text as the displays at that scale
  would (`may_open_on`, until it's on one); a popup takes its window's
  scale and display. app_server answers for the output a new window
  was placed on, a popup's window's or a layer's (not the first
  output).
- Glyphs drawn ahead (`TextSystem::warm`, `PlatformTextSystem::
  draw_ahead`): a face and size whose glyphs are running text (32 drawn
  the first time) has the rest of its printable ASCII and common
  punctuation drawn, at each subpixel offset, by one background thread
  that leaves after 10 s idle, on a rasterizer and hinting of its own
  (the text system isn't held while it draws; their pixels go in in one
  write), so a frame that shows them only copies them into its atlas
  (drawing one the first time costs a frame ~12 µs). An app asks for
  those it will soon need (`Window::warm_glyphs`: Telegram's
  conversation text at start), drawn again for each scale and text
  rendering the window moves to. Telegram's chat switches: slowest
  frame 9.6–10.2 ms before, 6.6–7.6 ms; 0.7 MB more memory.
- Olo's text (`gpui/src/platform/linux/text_system.rs`), the same as
  the egui kit's: Olo's own faces (data/fonts) before the system's,
  shaped by harfrust, outlines hinted by skrifa's autohinter (vertical
  only, "light") and filled to each pixel's exact coverage by an area
  rasterizer kept between glyphs (`fill`: 5.9 µs a glyph against
  vello_cpu's 7.6–9.7, whose coverage was up to 57 levels of grey off
  on curves; colour glyphs are still vello_cpu's); variable faces give every
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
- A display's size and place are xdg-output's, in points (upstream
  divided wl_output's mode by its whole-number scale: a 1.5× display was
  1280 points wide, not 1707), and its mode is the one wl_output marks
  current (upstream took the last it listed, so a display offering
  smaller modes was taken for one, and its refresh rate for theirs).
  `Window::display` answers before the window is on a display when only
  one is possible (the only display; a popup's window's): what a window
  opening sized to its content needs.
- A run of text at a size of its own (`TextRun::size`, shaped and
  painted at it within the line: code in a paragraph).
- Pictures drawn with nearest sampling (`Window::paint_image_nearest`,
  `PolychromeSprite::nearest`): a magnified small picture keeps its
  pixels.
- `Window::set_keep_awake`: the idle-inhibit protocol, the screen kept
  awake while the window shows (a playing video).
- A window's renderer is made once: at the device size its scale gives
  (the scale probe is answered first, a roundtrip of 0.1–0.2 ms) and
  with the transparency it keeps (opaque unless it asks for client-side
  decorations or a see-through background). Upstream made it at scale 1
  and transparent, then again opaque (another set of pipelines for the
  other alpha) and again at its size: 15–18 ms of each window's opening.
  The pipelines are made side by side, on scoped threads (10.4 → 4.5 ms).
- `Window::activation_token(done)`: an xdg-activation token from the
  window's last click, for another program to raise its window with (a
  notification's app). `Window::set_layer_keyboard` changes a layer
  surface's keyboard as it shows.
- Wrapping lets a space after a word hang past the edge (as typesetters,
  CSS and egui), rather than pushing the word before it down a row: a
  row's last word fits when the word does, and the next row begins with
  a word, not the space. Leading spaces (an indent) still count.
- A window destroys the objects made for it with it (its fractional
  scale, its xdg-dialog, its idle inhibitor; upstream left the first two
  alive, naming a surface or toplevel that was gone and whose id went to
  something else), and its frame loop stops when it closes (upstream
  drew once more for a window already gone, and asked its destroyed
  surface for a frame).
- `Window::draw_if_dirty`: draws what changed without showing it (the
  next frame does). A window that isn't showing draws nothing until it
  shows, so what agents read of it would be as last drawn.
- `WindowOptions::foreign_parent`: a toplevel over another program's
  window, by the handle it exported (xdg-foreign v2: a portal's dialogs
  over the app asking), the imported object let go with the window.
- A closed Wayland window lets go of its callbacks as it drops (keeping
  only the close callback the tidying task runs), so the views they hold
  are released in the same frame, not after the next event.
- Text in runs keeps each run's face: `WindowTextSystem::layout_line`
  merged runs of one colour and size whatever their font (it compared
  the last run's font with itself), so bold and italic in a line of one
  colour came out in the first run's face. `TextSystem::standalone()`
  and `layout_line_now` shape without a window (laying a document out
  for print on a worker); `preload_system_fonts` reads the system's
  fonts ahead, off the frame.
- `Window::paint_glyphs`: a run's glyphs in one go, as sprites batched
  into the scene at once, each glyph's atlas tile looked up once a
  frame (`glyph_memo`). `Window::paint_glyph_run(run, origin, stamp,
  glyphs)` keeps what it found in a `GlyphRun` (one a row of text): the
  same run painted again a whole number of device pixels away (scrolled)
  is its sprites moved, not tens of thousands of lookups a frame. Kept
  while the scale, the text rendering, the caller's stamp (its colours)
  and where the origin falls within a device pixel stay the same.
  Sprites sort by texture, not tile: a page of glyphs, in order already,
  sorts in a pass.
- `Window::note_scroll` (a view's offset, for `{"frames":true}`'s marks,
  so tools/dev/scroll-check and the bench see GPUI's scrolling) and
  `ScrollHandle::set_extent` (a view that scrolls itself tells its
  scroll bars how far it goes).
- The scene drops a big quad wholly under opaque quads drawn after it,
  and cuts a plain one mostly under them to what shows
  (`Scene::cull_hidden_quads`): a window's ground under its views' own
  filled the window two or three times a frame, the GPU's biggest cost
  (a full-screen StyledEdit at 1×: 2.3 → 1.3 ms of GPU a frame, the
  quads 2.01 → 1.01 screens).
- A window resized by hand (xdg's `resizing`) gets a swapchain as big
  as its screen on the press, before anything moves, and shows only its
  part of it (`wp_viewport`'s source, one buffer pixel to a device
  pixel); 1.5 s after the resize ends it is made to fit
  (`BladeRenderer::update_drawable_size_within`). A swapchain made is
  new GPU memory, 10 ms and more for a big one on Apple's GPU, and
  upstream made one for every size the window went through.
- `OLO_GPU_TIMINGS=1`: every pass timed on the GPU, and each frame's
  times, its quads (in screens filled) and sprites printed to stderr.
- Drops a window takes itself: what's dragged over a window
  (`Window::drop_offer`: its types, what its source allows, whether one
  of this program's windows dragged it out, its files) is offered to
  its parts as they paint; one that would take it says so each frame
  (`Window::accept_drop(target, mime, action)`: the compositor shows
  the drop's action from that), and what's dropped comes to the
  window's `on_dropped` handler, its data read for the type taken
  (`Dropped::paths` for files). A window with no handler keeps
  upstream's way (files, copied, through `on_drop::<ExternalPaths>`),
  and refuses this program's own drags (an app dragged out of Deskbar
  onto its own row); one with a handler takes them as it likes (files
  dragged from one of Tracker's windows to another).
- Elastic ends (`Elastic`), as on a Mac, for everything that scrolls
  (`div`'s `overflow_scroll` and `uniform_list` keep one per scroller;
  a view that scrolls itself keeps its own, as the GPUI kit's editor):
  fingers on a touchpad pull the content past an end against Apple's
  rubber band (a third of the view at most) and it springs back when
  they lift (a critically damped spring, most of the way back in a
  quarter of a second); the glide after them bounces off an end as fast
  as it came (a quarter of the view deep at most), the rest of it spent;
  a glide that takes over from fingers pulling past an end (a touchpad's
  glide follows them with no stop between: the Wayland client ends the
  fingers' scroll when it starts) goes on from where they left it. A wheel
  stops at the ends, and an axis with nothing to scroll doesn't
  stretch. `ScrollWheelEvent::kind` says which (`ScrollKind`: from the
  axis source, app_server sending its glide as `Continuous`).
- A frame asked for while one is being drawn (an animation's next
  frame: `request_animation_frame`, `on_next_frame`) comes with the
  display's next frame callback if this one commits, not at once: every
  animation drew twice a display frame (16.6 ms and 1 ms apart at 60
  Hz), and a live resize two frames a configure.
