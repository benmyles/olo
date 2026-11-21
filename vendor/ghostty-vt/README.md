# libghostty-vt

Vendored C headers and prebuilt static archive of Ghostty's `libghostty-vt`
library (MIT license; see `LICENSE`), used by `crates/rex-vt` (`olo-rex-vt`)
for lossless Ghostty `GHOSTSNP` incremental snapshot decoding, VT emulation,
Kitty graphics protocol, search, selection, and key/mouse encoding in Olo Rex.

- **Upstream:** https://github.com/ghostty-org/ghostty
- **Commit:** `6d244209978e599277904ff3ced7b8a2ebcc528d` (`libghostty-vt` `1.0.2-dev`)
- **Compiler:** Zig `0.17.1`
- **Build flags:** `-Demit-lib-vt=true -Doptimize=ReleaseFast -Dstrip=true` (SIMD enabled)
- **Runtime link dependencies:** `libc`, `libm` only (fully self-contained static archive).

To rebuild `lib/<arch>-linux-gnu/libghostty-vt.a` (or build for `aarch64`), run:

```bash
tools/build-ghostty-vt.sh
```
