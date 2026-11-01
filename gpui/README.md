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
- `Window::with_element_opacity` is public (a disabled control,
  faded), and popups (`WindowKind::Popup`) take an `offset` from their
  anchor (a menu field's menu, its current item over the field).
- GPU memory sized for many small windows on unified memory: the
  allocator's chunks start at 256 KB and stop at 2 MB, anything of 2 MB
  or more is its own allocation (blade's prototyping defaults grew
  chunks to 128 MB: hello_world held 96 MB, mostly empty), and the
  textures paths are drawn through (window-sized, one of them 4×
  multisampled) are made only when a window first draws a path.
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
  system's fonts load only when something needs them.
- Text drawn for the display a window is on (`TextRendering`, chosen by
  `olo_protocol::panels::text_mode` from the display's density, colour
  stripes and rotation): grey-scale unhinted at Retina density (as
  macOS), LCD subpixel coverage on low-density striped LCDs (FreeType's
  LCD filter, blended per channel with dual-source blending, which
  blade-graphics gained), hinted grey-scale in between. A window drawn
  again when it moves to a display that wants another.
  `set_display_text_rendering` takes the person's choice from
  Preferences ▸ Display; `OLO_TEXT_RENDERING` forces one for tests.
