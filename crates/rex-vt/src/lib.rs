//! Safe Rust wrapper around Ghostty's `libghostty-vt` for Olo Rex.
//!
//! Provides:
//! - Lossless incremental `GHOSTSNP` binary snapshot decoding ([`SnapshotDecoder`],
//!   [`try_decode_snapshot_prefix`]) matching Rex's `rex.data.terminal.v1` stream.
//! - Dirty-tracked [`RenderState`] with stable row IDs, overscan, graphemes,
//!   styles, cursor geometry, and Kitty graphics placements.
//! - Incremental scrollback [`TerminalSearch`], selection helpers, OSC 133
//!   prompt navigation, Ghostty & `.itermcolors` theme importers, and mouse/paste
//!   encoding.

pub mod color;
pub mod error;
pub mod input;
mod limits;
pub mod render;
pub mod search;
pub mod snapshot;
pub mod sys;
pub mod terminal;

#[allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code, clippy::all)]
pub mod ffi {
    include!(concat!(env!("OUT_DIR"), "/ghostty_vt_bindings.rs"));
}

pub use color::{Rgb, TerminalTheme, default_palette, generate_extended_palette, parse_color, parse_ghostty_theme, parse_itermcolors, parse_palette_entry};
pub use error::VtError;
pub use input::{KeyMods, MouseAction, MouseButton, MouseEncoder, MouseGeometry, encode_focus, encode_paste, paste_is_safe};
pub use render::{CellStyle, CellWide, CursorInfo, CursorStyle, DirtyState, ImageLayer, ImagePlacement, RenderCell, RenderColors, RenderRow, RenderState, RowId, RowRaw, UnderlineStyle};
pub use search::{SearchStatus, TerminalSearch, ViewportMatch};
pub use snapshot::{MAX_SNAPSHOT_BYTES, SnapshotDecoder, SnapshotFramer, SnapshotProgress, StreamingSnapshotResult, try_decode_snapshot_prefix};
pub use terminal::{DecodedKittyImage, FormatKind, GhosttyTerminal, ScreenKind, ScrollbarState, TerminalMemoryUsage, VtEngine};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_write_render_search_selection_and_snapshot_roundtrip() {
        let mut term = GhosttyTerminal::new(80, 24).expect("create terminal");
        term.vt_write(b"Hello, \x1b[1;32mOlo Rex\x1b[0m!\r\nSecond line with link: \x1b]8;;https://example.com\x1b\\Click\x1b]8;;\x1b\\\r\n");

        let mut rs = RenderState::new().expect("render state");
        rs.update(&term).expect("update");
        assert_eq!(rs.size(), (80, 24));
        assert_ne!(rs.dirty(), DirtyState::Clean);
        let colors = rs.colors();
        let rows = rs.extract_rows(&colors.palette, false).expect("rows");
        assert_eq!(rows.len(), 24);
        assert_eq!(rows[0].cells[0].codepoint, 'H' as u32);
        assert!(rows[0].cells[7].style.bold);
        assert!(rows[1].cells[23].hyperlink);
        assert_eq!(term.hyperlink_at_viewport(23, 1).as_deref(), Some("https://example.com"));

        rs.clean().expect("clean");
        rs.update(&term).expect("second update");
        assert_eq!(rs.dirty(), DirtyState::Clean);

        // Search
        let mut search = TerminalSearch::new(&term).expect("search");
        search.set_needle(Some("olo rex")).expect("needle");
        search.run().expect("run");
        assert_eq!(search.total_matches(), 1);
        assert!(search.select_next().expect("select"));
        let vmatches = search.viewport_matches(&term);
        assert_eq!(vmatches.len(), 1);
        assert!(vmatches[0].selected);

        // Selection
        assert!(term.select_word_at_viewport(11, 0).expect("select word"));
        assert_eq!(term.selection_text().expect("sel text").as_deref(), Some("Rex!"));
        assert!(term.has_selection());
        term.clear_selection().expect("clear sel");

        // Snapshot encode + streaming prefix decode with trailing live VT bytes
        let snap = term.encode_snapshot().expect("encode snapshot");
        assert!(snap.starts_with(b"GHOSTSNP"));

        // Truncated prefix returns Ok(None)
        assert!(try_decode_snapshot_prefix(&snap[..snap.len() / 2], true, None).expect("truncated").is_none());

        // Complete snapshot + trailing VT bytes returns exact consumed offset
        let mut combined = snap.clone();
        let trailing = b"\x1b[3;1HTrailing line";
        combined.extend_from_slice(trailing);
        let decoded = try_decode_snapshot_prefix(&combined, true, None).expect("decode").expect("complete");
        assert_eq!(decoded.consumed, snap.len());
        let mut restored = decoded.terminal;
        restored.vt_write(&combined[decoded.consumed..]);
        let plain = restored.format(FormatKind::Plain, true, true).expect("format");
        assert!(plain.contains("Hello, Olo Rex!"));
        assert!(plain.contains("Trailing line"));
    }

    #[test]
    fn ghostty_and_itermcolors_theme_importers() {
        let g = parse_ghostty_theme(
            "TestGhostty",
            "
            background = #011627
            foreground = #d6deeb
            cursor-color = #80a4c2
            palette = 0=#011627
            palette = 1=#ef5350
            ",
        )
        .expect("parse ghostty");
        assert_eq!(g.background, Rgb::new(0x01, 0x16, 0x27));
        assert_eq!(g.foreground, Rgb::new(0xd6, 0xde, 0xeb));
        assert_eq!(g.palette[1], Rgb::new(0xef, 0x53, 0x50));
        let generated = g.with_generated_extended_palette(true);
        assert!(generated.extended_palette);
        assert_eq!(generated.palette.len(), 256);

        let iterm = parse_itermcolors(
            "TestIterm",
            r#"<plist><dict>
                <key>Background Color</key><dict>
                    <key>Red Component</key><real>0.0</real>
                    <key>Green Component</key><real>0.5</real>
                    <key>Blue Component</key><real>1.0</real>
                </dict>
                <key>Ansi 1 Color</key><dict>
                    <key>Red Component</key><real>1.0</real>
                    <key>Green Component</key><real>0.0</real>
                    <key>Blue Component</key><real>0.0</real>
                </dict>
            </dict></plist>"#,
        )
        .expect("parse iterm");
        assert_eq!(iterm.background, Rgb::new(0, 128, 255));
        assert_eq!(iterm.palette[1], Rgb::new(255, 0, 0));
    }

    #[test]
    fn paste_safety_and_encoding() {
        assert!(paste_is_safe("hello world"));
        assert!(!paste_is_safe("rm -rf /\n"));
        assert!(!paste_is_safe("foo\x1b[201~bar"));
        let enc = encode_paste("echo hi\n", true).expect("bracketed");
        assert!(enc.starts_with(b"\x1b[200~"));
        assert!(enc.ends_with(b"\x1b[201~"));
    }
}
