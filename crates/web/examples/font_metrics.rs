//! Print a font file's vertical metrics (hhea, OS/2 typo and win, the flag
//! that selects typo) raw and at a CSS pixel size, with the integer-rounded
//! ascent, descent and line gap Chrome uses for `line-height: normal`, for
//! comparing the engine's line heights with Chrome's. WOFF2 and WOFF are
//! decoded; a variable font's default instance is measured.
//!
//!   cargo run --release -p olo-web --example font_metrics -- FONT [SIZE]
use std::path::Path;
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let file = Path::new(&args[0]);
    let size: f64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(16.);
    let bytes = std::fs::read(file).expect("read font");
    let bytes = olo_web::font::woff::decode(&bytes, None).unwrap_or(bytes);
    let face = ttf_parser::Face::parse(&bytes, 0).expect("parse font");
    let upem = f64::from(face.units_per_em());
    let t = face.tables();
    let h = t.hhea;
    println!("units per em {upem}; hhea ascender {} descender {} line gap {}", h.ascender, h.descender, h.line_gap);
    if let Some(o) = t.os2 {
        println!(
            "OS/2 typo ascender {} descender {} line gap {} (USE_TYPO_METRICS {}); win ascent {} descent {}",
            o.typographic_ascender(),
            o.typographic_descender(),
            o.typographic_line_gap(),
            o.use_typographic_metrics(),
            o.windows_ascender(),
            o.windows_descender()
        );
    }
    let (a, d, g) = (f64::from(face.ascender()), f64::from(face.descender()), f64::from(face.line_gap()));
    let scale = size / upem;
    println!("ttf-parser policy at {size}px: ascent {:.3} descent {:.3} line gap {:.3} → normal line height {:.3}", a * scale, -d * scale, g * scale, (a - d + g) * scale);
    let (ra, rd, rg) = ((a * scale).round(), (-d * scale).round(), (g * scale).round());
    println!("rounded as Chrome: ascent {ra} descent {rd} line gap {rg} → normal line height {}", ra + rd + rg);
    if let Some(gid) = face.glyph_index('0') {
        println!("ch (advance of '0') {:.3}px", f64::from(face.glyph_hor_advance(gid).unwrap_or(0)) * scale);
    }
}
