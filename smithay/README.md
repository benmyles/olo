# Olo's Smithay

The compositor library app_server is built on. It began as Smithay 0.7.0
at upstream's commit 24647fc8d00131eb7f388efed26e35ea316b0018 (Smithay's
authors, under the MIT licence, [smithay/LICENSE.txt](smithay/LICENSE.txt);
https://github.com/Smithay/smithay), and is Olo's own now, like
[GPUI](../gpui/README.md): changed as Olo needs, never synced with
upstream.

- `smithay`: the library (`src/`, with its `build.rs` and the two C
  probes it compiles to see what the system's gbm can do).
- `smithay-drm-extras`: EDID and display-info helpers, as published.

Upstream's examples (anvil, smallvil), its test clients, benches and
docs aren't here: app_server (`crates/app_server`) is Olo's anvil.

What Olo changed, so far (see "Every frame on time" in
[docs/HACKING.md](../docs/HACKING.md)):

- No default features: Olo is Wayland only, and a workspace build
  unifies a package's features across members, so upstream's default
  (X11, Xwayland, winit with X11) reached app_server. It names what it
  uses. And winit itself is asked for Wayland alone (`Cargo.toml`:
  upstream's `winit` dependency had its "x11" feature too, so
  `backend_winit`, which app_server's nested development run uses,
  still built winit-x11, x11rb and x11-dl into app_server until
  2026-11-12). The X11 backend and Xwayland sources are upstream's,
  behind features nothing turns on: not built, kept so the tree stays
  upstream's to merge with (their optional `x11rb` is why that name is
  still in Cargo.lock, in no build).
- Where a frame's time goes is told to `backend::renderer::PHASE_HOOK`:
  the DRM compositor's "planes", "atomic test" and "render_output", the
  damage tracker's "draws" and "finish" (the flush to the GPU and its
  fence).
- An element refused an overlay plane without a test says why
  (`RenderingReason::Overlaps`, `Underlay`, `PlaneTaken`; upstream gave
  no reason), so a frame's record can say what was drawn and why.
- A window's own surfaces are `Kind::ScanoutCandidate` (upstream:
  `Unspecified`, which the compositor never tries on a plane), so a
  window whose buffer changes every frame can go straight to an overlay
  plane. Its popups stay as they were.
- The cursor plane is kept on the display: clipped at an edge (the
  destination the part of the pointer on the display, the source the
  same part of its buffer), tested when its size changes, no plane at
  all when the pointer is wholly off the display. apple-drm's overlay
  plane standing in for a cursor plane refused one hanging off the
  display with EINVAL.
- Locks in one order (found by lockbud and olo-sync's tracing-mutex in
  debug builds): the popup manager's tree lock is a leaf, nothing else
  locked under it and it never under a surface's (`PopupManager`
  locked a toplevel's surface then the tree to add a popup, and the
  tree then each popup's surface to find one: a submenu closed the
  cycle, and a test session's app_server stopped there); the DRM
  surfaces' `commit_pending` takes the state then the pending lock, as
  everything else does, and `vrr_supported` reads its lock once.
- A GPU plugged in on another seat isn't this seat's: `UdevBackend`
  passes on a device added only when its `ID_SEAT` is the backend's, as
  it already chose the GPUs it starts with (upstream offered every new
  card, and logind refused app_server the other seat's, which it logged
  as an error: found loading vkms on a seat of its own for a test).
- The damage tracker warns of a step of a frame over 4 ms: beginning
  it, an element's draw (naming the element, its place and kind), or
  finishing it (`slow frame: …`), so a slow frame says where it went.
- The GLES renderer keeps the last read-back's buffer (`GlesCleanup`'s
  spare) for the next read-back of its size, a screenshot's 33 MB at 4K,
  rather than making and freeing one each time; let go 10 s after its
  last use.
- Its doctests build in `cargo test --workspace`: it's its own
  dev-dependency with `renderer_test`, the dummy renderer they use
  (`cfg(doctest)` doesn't reach the library, rust-lang/rust#67295).
  Only test builds get it.
