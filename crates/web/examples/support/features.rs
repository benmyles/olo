//! Compile-time switches recorded by release measurement executables.
pub fn enabled() -> Vec<&'static str> {
    [
        ("scalar-image-processing", cfg!(feature = "scalar-image-processing")),
        ("scalar-scans", cfg!(feature = "scalar-scans")),
        ("baseline-storage", cfg!(feature = "baseline-storage")),
        ("baseline-parser", cfg!(feature = "baseline-parser")),
        ("baseline-css-parser", cfg!(feature = "baseline-css-parser")),
        ("baseline-css-byte-copy", cfg!(feature = "baseline-css-byte-copy")),
        ("baseline-utf8-errors", cfg!(feature = "baseline-utf8-errors")),
        ("baseline-selector-nth", cfg!(feature = "baseline-selector-nth")),
        ("baseline-color-f64", cfg!(feature = "baseline-color-f64")),
        ("baseline-color-conversion", cfg!(feature = "baseline-color-conversion")),
        ("baseline-math-folding", cfg!(feature = "baseline-math-folding")),
        ("baseline-math-sums", cfg!(feature = "baseline-math-sums")),
        ("baseline-cascade-index", cfg!(feature = "baseline-cascade-index")),
        ("baseline-style-dense", cfg!(feature = "baseline-style-dense")),
        ("baseline-cascade-value-copy", cfg!(feature = "baseline-cascade-value-copy")),
        ("baseline-font-family-fat", cfg!(feature = "baseline-font-family-fat")),
        ("baseline-text-ascii", cfg!(feature = "baseline-text-ascii")),
        ("baseline-text-prepare", cfg!(feature = "baseline-text-prepare")),
        ("baseline-text-map-wide", cfg!(feature = "baseline-text-map-wide")),
        ("baseline-linebreak-ascii", cfg!(feature = "baseline-linebreak-ascii")),
        ("baseline-linebreak-runs", cfg!(feature = "baseline-linebreak-runs")),
        ("baseline-linebreak-class-table", cfg!(feature = "baseline-linebreak-class-table")),
        ("baseline-inline-reshape", cfg!(feature = "baseline-inline-reshape")),
    ]
    .into_iter()
    .filter_map(|(name, on)| on.then_some(name))
    .collect()
}
