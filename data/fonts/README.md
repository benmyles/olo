# Olo's fonts

The faces Olo draws its own text in, shipped with it so every machine
looks the same (see "Text" in "Look and feel rules", docs/HACKING.md).
All but Sora are from the Noto project's 2025.12.01 release; all are
unchanged and under the SIL Open Font License 1.1 (OFL.txt; Sora's own
copyright notice in OFL-Sora.txt):

| File | Family | Version | Source |
|---|---|---|---|
| `NotoSans[wght].ttf` | Noto Sans, every weight | 2.015 | notofonts/latin-greek-cyrillic |
| `NotoSans-Italic[wght].ttf` | Noto Sans Italic, every weight | 2.015 | notofonts/latin-greek-cyrillic |
| `NotoSansMono[wght].ttf` | Noto Sans Mono, every weight | 2.014 | notofonts/latin-greek-cyrillic |
| `NotoSansSymbols[wght].ttf` | Noto Sans Symbols (arrows, ⌃) | 2.003 | notofonts/symbols |
| `NotoSansSymbols2-Regular.ttf` | Noto Sans Symbols 2 (⌘ ⌥ ⇧ ⌫, ▸, ✓) | 2.008 | notofonts/symbols |
| `NotoSansMath-Regular.ttf` | Noto Sans Math (more arrows, ≈) | 3.000 | notofonts/math |
| `Sora[wght].ttf` | Sora, every weight: Olo's brand face (docs/BRAND.md), for the wordmark and Olo's name where it stands as the brand, never for UI text | 2.000 | google/fonts `ofl/sora` (sora-xor/sora-font) |

The `[wght]` files are variable: one file has every weight, and the kit
and app_server pick the weight on its axis.
