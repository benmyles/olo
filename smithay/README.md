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

What Olo changed, so far:

- (Nothing yet beyond the move; the DRM compositor's plane handling and
  frame timing are first in line: see "Every frame on time" in
  [docs/HACKING.md](../docs/HACKING.md) and `apple-drm` in
  [docs/TODO.md](../docs/TODO.md).)
