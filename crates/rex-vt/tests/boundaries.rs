//! Exercise the public wrapper with in-memory terminals only.
use olo_rex_vt::*;

#[test]
fn grid_and_pixel_limits_preserve_the_previous_screen() {
    assert!(matches!(GhosttyTerminal::new(0, 8), Err(VtError::InvalidValue)));
    assert!(matches!(GhosttyTerminal::new(u16::MAX, u16::MAX), Err(VtError::LimitExceeded)));
    let mut t = GhosttyTerminal::new(120, 32).unwrap();
    assert_eq!(t.resize(120, 0, 1, 1), Err(VtError::InvalidValue));
    assert_eq!(t.resize(2048, 2048, 1, 1), Err(VtError::LimitExceeded));
    assert_eq!(t.resize(120, 32, u32::MAX, 12), Err(VtError::LimitExceeded));
    assert_eq!(t.size(), (120, 32));
    t.vt_write(b"\x1b[?2004h");
    assert!(t.bracketed_paste_mode());
    assert!(!t.dec_mode(0x8000 | 2004));
    assert_eq!(ScrollbarState { total: 10, offset: u64::MAX, len: 1 }.display_offset_from_bottom(), 0);
}

#[test]
fn snapshot_boundaries_and_poisoned_decoder() {
    let t = GhosttyTerminal::new(8, 2).unwrap();
    let bytes = t.encode_snapshot().unwrap();
    for end in 0..bytes.len() {
        assert!(try_decode_snapshot_prefix(&bytes[..end], true, None).unwrap().is_none());
    }
    let mut bad = bytes.clone();
    // Damage just the checksum of an otherwise complete FINISH record.
    *bad.last_mut().unwrap() ^= 1;
    assert!(try_decode_snapshot_prefix(&bad, true, None).is_err());
    let mut d = SnapshotDecoder::new(bad, true, None).unwrap();
    assert_eq!(d.next(), Err(VtError::InvalidValue));
    let _term = d.ready().unwrap();
    assert!(d.ready().is_err());
    assert!(d.next().is_err());
    assert_eq!(d.source_offset(), Err(VtError::InvalidValue));
    assert_eq!(d.next(), Err(VtError::InvalidValue));
    assert_eq!(d.history_rows_primary(), None);

    assert!(matches!(SnapshotDecoder::new(bytes.clone(), true, Some(u64::MAX)), Err(VtError::LimitExceeded)));
    let mut bad = bytes.clone();
    bad[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(try_decode_snapshot_prefix(&bad, true, None), Err(VtError::LimitExceeded)));
    let mut bad = bytes.clone();
    bad[20..24].copy_from_slice(&[255; 4]);
    assert!(matches!(SnapshotDecoder::new(bad, true, None), Err(VtError::LimitExceeded)));
    let mut d = SnapshotDecoder::new(bytes, true, None).unwrap();
    drop(d.ready().unwrap());
    assert_eq!(d.next().unwrap(), None);
    assert_eq!(d.next().unwrap(), None);
}

#[test]
fn decoder_and_search_can_move_independently() {
    let mut source = GhosttyTerminal::new(40, 6).unwrap();
    for _ in 0..900 {
        source.vt_write(b"search me\r\n");
    }
    let mut d = SnapshotDecoder::new(source.encode_snapshot().unwrap(), true, None).unwrap();
    let mut t = d.ready().unwrap();
    let mut search = TerminalSearch::new(&t).unwrap();
    search.set_needle(Some("search")).unwrap();
    std::thread::scope(|scope| {
        scope.spawn(move || while d.next().unwrap().is_some() {});
        scope.spawn(|| {
            for _ in 0..50 {
                search.run().unwrap();
            }
        });
        for _ in 0..50 {
            t.vt_write(b"more\r\n");
        }
    });
    t.reset();
    t.vt_write(b"search");
    assert_eq!(search.viewport_matches(&t).len(), 1);
    assert!(search.viewport_matches(&source).is_empty());
    drop(t);
    assert_eq!(search.run(), Err(VtError::InvalidValue));
}

#[test]
fn incompletely_written_vt_resumes_and_random_input_stays_renderable() {
    let mut t = GhosttyTerminal::new(24, 3).unwrap();
    t.vt_write(b"\xff\xe2");
    t.vt_write(b"\x1b]0;unfinished title");
    t.vt_write(b"\x1b\\\x1b[Hok");
    assert!(t.viewport_row_text(0).starts_with("ok"));
    let mut seed = 123456789u32;
    for _ in 0..200 {
        let mut bytes = [0; 80];
        for b in &mut bytes {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *b = seed as u8;
        }
        t.vt_write(&bytes);
        assert!(try_decode_snapshot_prefix(&bytes, true, None).is_err());
    }
    t.reset();
    t.vt_write(b"working");
    assert_eq!(t.viewport_row_text(0), "working");
}

#[test]
fn kitty_pixels_format_limits_and_missing_images() {
    let mut t = GhosttyTerminal::new(20, 4).unwrap();
    t.resize(20, 4, 8, 16).unwrap();
    // A valid 1x1 RGBA PNG containing [17, 34, 51, 255].
    t.vt_write(b"\x1b_Ga=T,f=100,i=9;iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGMQVDL+DwACFAFmBODefwAAAABJRU5ErkJggg==\x1b\\");
    let img = t.kitty_image_rgba(9).unwrap();
    assert_eq!((img.width, img.height), (1, 1));
    assert_eq!(img.rgba, [17, 34, 51, 255]);
    assert!(t.memory_usage().image_bytes >= 4);
    t.vt_write(b"\x1b_Ga=t,f=24,s=1,v=1,i=10;ESIz\x1b\\");
    assert_eq!(t.kitty_image_rgba(10).unwrap().rgba, [17, 34, 51, 255]);
    t.vt_write(b"\x1b_Ga=t,f=100,i=11;bm90IGFuIGltYWdl\x1b\\");
    assert!(t.kitty_image_rgba(11).is_none());
    t.vt_write(b"\x1b_Ga=t,f=100,i=12;iVBORw0KGgoAAAANSUhEUgAAQAEAAAABCAYAAADJXd1mAAAADUlEQVR4nGMQVDL+DwACFAFmBODefwAAAABJRU5ErkJggg==\x1b\\");
    assert!(t.kitty_image_rgba(12).is_none());
    let mut state = RenderState::new().unwrap();
    state.update(&t).unwrap();
    assert!(state.extract_image_placements().unwrap().iter().any(|p| p.image_id == 9));
    let snap = t.encode_snapshot().unwrap();
    let (t, _) = SnapshotDecoder::decode_all(snap, true, Some(0)).unwrap();
    assert!(t.kitty_image_rgba(9).is_none());
    assert_eq!(t.memory_usage().image_bytes, 0);
}

#[test]
fn theme_limits_and_color_only_files() {
    let oversized = " ".repeat(color::MAX_THEME_BYTES + 1);
    assert!(parse_ghostty_theme("test", &oversized).is_err());
    assert!(parse_itermcolors("test", &oversized).is_err());
    assert!(parse_ghostty_theme("test", "cursor-color = nonsense").is_err());
    assert_eq!(parse_ghostty_theme("test", "selection-background = #ff0000").unwrap().selection_bg, Some(Rgb::new(255, 0, 0)));
    let xml = "<key>Selection Color</key><dict><key>Red Component</key><real>1</real><key>Green Component</key><real>0</real><key>Blue Component</key><real>0</real></dict>";
    assert_eq!(parse_itermcolors("test", xml).unwrap().selection_bg, Some(Rgb::new(255, 0, 0)));
    assert!(parse_itermcolors("test", &xml.replace("<real>1", "<real>NaN")).is_err());
    let mut theme = TerminalTheme::default();
    theme.palette.truncate(1);
    theme.palette[0] = Rgb::new(1, 2, 3);
    let mut t = GhosttyTerminal::new(40, 10).unwrap();
    t.apply_theme(&theme).unwrap();
    let mut r = RenderState::new().unwrap();
    r.update(&t).unwrap();
    assert_eq!(r.colors().palette, theme.palette_256());
}

#[test]
fn repeat_cells_dirty_rows_and_two_phase_render() {
    let mut t = GhosttyTerminal::new(12, 4).unwrap();
    t.vt_write("XXe\u{301}e\u{300}\r\n\x1b[31mzz".as_bytes());
    let mut r = RenderState::new().unwrap();
    r.begin_update(&t).unwrap();
    let palette = r.colors().palette;
    let rows = r.extract_rows(&palette, true).unwrap();
    assert_eq!(rows[1].cells[0], rows[1].cells[1]);
    assert_ne!(rows[0].cells[2].grapheme, rows[0].cells[3].grapheme);
    r.end_update().unwrap();
    r.clean().unwrap();
    t.vt_write(b"more");
    r.update(&t).unwrap();
    let rows = r.extract_rows(&palette, true).unwrap();
    assert!(rows[0].cells.is_empty());
    assert!(!rows[1].cells.is_empty());
    assert_eq!(r.set_overscan(u16::MAX, 0), Err(VtError::LimitExceeded));
}

#[test]
fn mouse_rejects_unrepresentable_geometry_and_positions() {
    let mut mouse = MouseEncoder::new().unwrap();
    for x in [f32::INFINITY, f32::NAN, f32::MAX] {
        assert_eq!(mouse.encode(MouseAction::Motion, None, KeyMods::NONE, x, 1.0), Err(VtError::InvalidValue));
    }
    assert_eq!(
        mouse.set_geometry(MouseGeometry { screen_width: 10, screen_height: 10, cell_width: 1, cell_height: 1, padding_top: 0, padding_bottom: 0, padding_left: u32::MAX, padding_right: 2 }),
        Err(VtError::InvalidValue)
    );
}

#[test]
fn incremental_framing_tracks_chunks_and_excludes_live_data() {
    let t = GhosttyTerminal::new(72, 24).unwrap();
    let mut bytes = t.encode_snapshot().unwrap();
    let end = bytes.len();
    bytes.extend_from_slice(b"hello");
    let mut framing = SnapshotFramer::default();
    for i in 0..end {
        assert_eq!(framing.advance(&bytes[..i]), Ok(None));
    }
    assert_eq!(framing.advance(&bytes), Ok(Some(end)));
    assert_eq!(framing.advance(&bytes[..end]), Ok(Some(end)));
    assert_eq!(framing.advance(&bytes[..8]), Err(VtError::InvalidValue));
}

#[test]
fn cursor_outside_viewport_and_optional_colors() {
    let mut t = GhosttyTerminal::new(16, 2).unwrap();
    for _ in 0..10 {
        t.vt_write(b"line\r\n");
    }
    t.scroll_to_top().unwrap();
    let mut r = RenderState::new().unwrap();
    r.update(&t).unwrap();
    let cursor = r.cursor();
    assert!(cursor.viewport_pos.is_none());
    assert!(!cursor.wide_tail);
    let rows = r.extract_rows(&r.colors().palette, false).unwrap();
    assert!(rows[0].cells.iter().all(|cell| cell.bg.is_none()));
}
