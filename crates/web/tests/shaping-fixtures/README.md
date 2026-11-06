# Native shaping references

Four **unmodified** Noto variable fonts (Arabic, Hebrew, Devanagari and Thai)
come from google/fonts commit `24ddc1f324cac5d9f5aedca126d298f56e3f1640`.
SOURCE.json records original paths, source URLs and SHA-256 hashes. Each family
includes its original Open Font License. The existing repository Noto Sans and
original native-font fixtures supply Latin/Greek/Cyrillic, optical sizing and
shared-table TTC data; they are read in place, not copied.

`generate.py` reads these files and calls independent **native HarfBuzz 15.0.0**
through the C API in `native_harfbuzz.py`. It never imports or executes Olo.
Explicit script/direction/language, variations, feature values and UTF-8 item
ranges produce 24 literal glyph vectors and 15 longer benchmark hashes.
Each packed hash encodes glyph ID, absolute UTF-8 cluster, four signed advances/
offsets and unsafe-to-break flag as little-endian `<IIiiiiB`. No struct padding,
CSS layout, automatic fallback, screenshots or engine output supplies the oracle.

The isolated generator environment requires fontTools **4.60.1**, uharfbuzz
**0.55.0** and native HarfBuzz **15.0.0**. The older binding embeds **14.2.1**;
those vectors are retained separately as version-comparison evidence. The
native 15 library is loaded independently; `OLO_SHAPING_HARFBUZZ_LIBRARY` can
point at an isolated library. Do not install/upgrade Olo's session to run it.
A Hebrew mark-offset difference against 14.2.1 vanishes against 15.0.0. Also,
the Hebrew file's fvar default is **weight 100**, and its first legacy name
contains “Thin”. Our explicit CSS-normal reference requests weight **400**;
it does not assume file defaults or first legacy names equal CSS defaults.

```sh
/var/tmp/OWN-DIRECTORY/venv/bin/python crates/web/tests/shaping-fixtures/generate.py
/usr/bin/python3 tools/browser/generate-shaping-unicode.py
cargo fmt -p olo-web
cargo test --release -p olo-web --test shaping
```

`ucd` contains unchanged Unicode **18.0.0** input and Unicode License V3. The
original factual generator produces bidi classes, paired brackets (including
canonical angle-bracket equivalence), default ignorables, grapheme classes,
Indic conjunct classes and extended-pictographic ranges. It consults no engine
or test output. All unassigned bidi defaults come from the file's `@missing`
rules before explicit assignments; surrogate codepoints do not enter Rust str.
Hangul syllable classes use the standard arithmetic rule in the iterator.

The entire **91,707-case BidiCharacterTest** checks paragraph base, line levels
and visual ordering through L2. Its own header excludes P1, L3 and L4; this is
not a claim about the full bidi/CSS pipeline. Paragraph levels precede L1, so
reference line levels are compared after L1. The entire **853-case
GraphemeBreakTest** checks the original forward extended-grapheme iterator.
Unicode 18 changes GB9c to `Linker Extend* × Consonant`; the older segmentation
crate missed ten supplied cases. The engine uses pinned current properties and
its own constant-state iterator, not that older segmentation implementation.

Raw font-unit glyph agreement does not prove rasterization, CSS spacing,
styled inline shaping, script-extension/paired-punctuation itemization, final
line breaking/reshaping, font sanitization, accessibility or website rendering.
