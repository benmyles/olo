# Olo: rules for agents

Read [docs/HACKING.md](docs/HACKING.md) before changing anything. It is the
source of truth for layout, the kit, look-and-feel rules and testing. These
rules are universal and non-negotiable:

- **BeOS-inspired, not a BeOS replica.** For everything you design, first
  look at how BeOS did it (the Be Book, in
  [vendor/bebook](vendor/bebook/README.md); Tracker; the kits) and keep
  whatever still makes sense in 2026 for what we're building; where there
  is a better modern way, use that instead.
- **Agent-first, as much as people-first.** Everything should work
  seamlessly for agents as well as for people: what a person can see and
  do, an agent can read (structured, not just pixels) and drive through a
  documented API, without getting in the person's way.
- **Never block the UI.** Anything that can take more than a frame runs on a
  worker thread, shows live progress, streams results and can be cancelled.
  See "The responsiveness rule" in HACKING.md.
- **Pixel-perfect in light, dark and night modes at 2× and other scales.**
  Colours from the palette, spacing from `metrics`, text via `style`.
- **Wayland only, no X11.** Olo runs Wayland programs and nothing
  else: no Xwayland, no X11 window management, no `DISPLAY`, no code or
  dependencies for X11-only programs. Programs that can speak either
  (Chrome, GTK, Qt) are run as Wayland clients.
- **Everything native.** Never wrap pre-existing system tools or apps
  (no shelling out to ffmpeg, pdftoppm, xdg-open, unzip, `op`, …). Read
  formats, talk protocols and do the work in Rust, in process (any
  Rust/cargo crate is fine), so the whole desktop is one cohesive,
  consistent experience.
- **Build on the shared kit.** If an app needs a widget or behaviour another
  app could use, add it to `crates/kit` and document it in HACKING.md.
- **Survive in-place reload.** Apps reconnect when app_server restarts and
  re-exec after an upgrade; implement `App::save_state` /
  `cx.restored_state()` so windows come back as they were. Test with
  `tsession start NAME --session` + rebuild + `tsession reload NAME`.
- **Icons** follow [docs/ICONS.md](docs/ICONS.md) (sunburst at max quality,
  against the reference set).
- **Test headless** with `tools/dev/tsession`; never open windows on the
  user's display. Journal visible changes with `tools/journal.py`.
