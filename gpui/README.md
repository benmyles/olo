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
  `gpui/src/platform/linux/wayland/window.rs`.
- GPU memory sized for many small windows on unified memory: the
  allocator's chunks start at 256 KB and stop at 2 MB, anything of 2 MB
  or more is its own allocation (blade's prototyping defaults grew
  chunks to 128 MB: hello_world held 96 MB, mostly empty), and the
  textures paths are drawn through (window-sized, one of them 4×
  multisampled) are made only when a window first draws a path.
- Background threads start when there's work and none is free, and
  leave after 10 s without any (upstream kept one per core).
