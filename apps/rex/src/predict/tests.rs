use super::*;

const WIDTH: usize = 40;

/// A screen of forty columns: `text` from the row's start, the
/// terminal's cursor after it.
fn screen(text: &str) -> Screen {
    at(text, text.chars().count() as u16)
}

fn at(text: &str, col: u16) -> Screen {
    let mut cells: Vec<Cell> = text.chars().map(Cell::of).collect();
    cells.resize(WIDTH, Cell::BLANK);
    Screen { row: 5, col, cells, drawn: false, shape: 0, alt: false }
}

/// The same, where the program draws its caret (in reverse video) and
/// hides the terminal's.
fn drawn(text: &str, col: u16) -> Screen {
    let mut s = at(text, col);
    s.drawn = true;
    s.cells[col as usize].ground = true;
    s
}

fn dimmed(mut s: Screen, from: usize, to: usize) -> Screen {
    for cell in &mut s.cells[from..to] {
        cell.dim = true;
    }
    s
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// A predictor that has seen a slow echo and one letter come true, at
/// a prompt: "$ x" on the screen, nothing waiting, typing shown.
fn warm() -> (Predictor, Instant) {
    let mut p = Predictor::new(true);
    let t = Instant::now();
    p.frame(Some(&screen("$ ")), t);
    let (seq, changed) = p.typed(b"x", Some(&screen("$ ")), t);
    assert!(!changed && p.overlay().is_none(), "the first of an epoch is unseen");
    p.answered(seq, t + ms(150));
    assert!(!p.frame(Some(&screen("$ x")), t + ms(300)), "nothing was drawn, nothing changes");
    assert_eq!(p.echo(), Some(ms(300)));
    (p, t + ms(1000))
}

/// What is drawn: the letters (a `_` for an empty cell, `|` for a
/// caret the program draws) from their column, and where the caret is.
fn shown(p: &Predictor) -> Option<(String, u16)> {
    p.overlay().map(|o| {
        let text: String = o
            .cells
            .iter()
            .map(|(c, like)| {
                if *like == Like::Caret {
                    '|'
                } else if *c == ' ' {
                    '_'
                } else {
                    *c
                }
            })
            .collect();
        (format!("{text}@{}", o.col), o.caret)
    })
}

/// The host has answered for all that was typed.
fn taken(p: &mut Predictor, t: Instant) {
    let all = p.writes;
    p.answered(all, t);
}

/// All that was typed has left for the host.
fn gone(p: &mut Predictor, t: Instant) {
    let all = p.writes;
    p.went(all, t);
}

fn type_all(p: &mut Predictor, keys: &[&[u8]], on: &Screen, t: Instant) -> u64 {
    keys.iter().map(|k| p.typed(k, Some(on), t).0).last().unwrap()
}

#[test]
fn letters_are_shown_as_typed_once_one_has_come_true() {
    let (mut p, t) = warm();
    // Typed on "$ x": each letter at once, the cursor after it.
    assert!(p.typed(b"l", Some(&screen("$ x")), t).1);
    assert_eq!(shown(&p), Some(("l@3".into(), 4)));
    assert!(p.typed(b"s", Some(&screen("$ x")), t + ms(100)).1);
    assert_eq!(shown(&p), Some(("ls@3".into(), 5)));
    // The host's echo of the first: it takes its place, the second still drawn.
    assert!(p.frame(Some(&screen("$ xl")), t + ms(350)));
    assert_eq!(shown(&p), Some(("s@4".into(), 5)));
    // And of the second: nothing is drawn, the frame is the picture.
    assert!(p.frame(Some(&screen("$ xls")), t + ms(450)));
    assert_eq!(p.overlay(), None);
    assert!(!p.waits());
    // After a pause, at the same place: still at once.
    assert!(p.typed(b" ", Some(&screen("$ xls")), t + ms(60_000)).1);
}

#[test]
fn frames_skip_and_repeat_and_rows_move() {
    let (mut p, t) = warm();
    for (i, letter) in [b"e", b"c", b"h", b"o"].iter().enumerate() {
        p.typed(*letter, Some(&screen("$ x")), t + ms(50 * i as u64));
    }
    assert_eq!(shown(&p), Some(("echo@3".into(), 7)));
    // A frame that shows nothing new for us, the row somewhere else in
    // the pane (what is above it grew): all still drawn, there.
    let mut moved = screen("$ x");
    moved.row = 9;
    p.frame(Some(&moved), t + ms(210));
    assert_eq!(p.overlay().map(|o| (o.row, o.col, o.caret)), Some((9, 3, 7)));
    // Three come back at once.
    assert!(p.frame(Some(&screen("$ xech")), t + ms(400)));
    assert_eq!(shown(&p), Some(("o@6".into(), 7)));
    // What the shell draws right of the cursor (a suggestion) is its own.
    let mut suggested = screen("$ xecho");
    for (i, c) in " hello".chars().enumerate().skip(1) {
        suggested.cells[7 + i] = Cell { ch: c, dim: true, ground: false };
    }
    assert!(p.frame(Some(&suggested), t + ms(500)));
    assert_eq!(p.overlay(), None);
    assert!(!p.waits());
}

#[test]
fn backspace_takes_back_what_was_typed_in_the_run_and_no_more() {
    let (mut p, t) = warm();
    let on = screen("$ x");
    p.typed(b"a", Some(&on), t);
    p.typed(b"b", Some(&on), t);
    assert!(p.typed(&[0x7f], Some(&on), t).1);
    assert_eq!(shown(&p), Some(("a_@3".into(), 4)), "the b is drawn over with nothing");
    // The host shows a, then ab, then a again: each a screen on the
    // way, and through all three what is seen is "a" with the cursor
    // after it (the b, when the host shows it, is drawn over).
    for (host, at_ms) in [("$ xa", 300), ("$ xab", 350)] {
        p.frame(Some(&screen(host)), t + ms(at_ms));
        assert!(p.waits(), "{host}");
        let o = p.overlay().expect("the rest is still drawn");
        assert_eq!(o.caret, 4, "{host}: {o:?}");
    }
    p.answered(3, t + ms(380));
    p.frame(Some(&screen("$ xa")), t + ms(400));
    assert!(!p.waits());
    // Twice more takes the a and the x back (typed in this run), shown at once…
    assert!(p.typed(&[0x7f], Some(&screen("$ xa")), t + ms(1000)).1);
    assert!(p.typed(&[0x7f], Some(&screen("$ xa")), t + ms(1000)).1);
    assert_eq!(shown(&p), Some(("__@2".into(), 2)));
    p.frame(Some(&screen("$ ")), t + ms(1300));
    assert!(!p.waits());
    // …and once more would take the prompt's own space: not drawn
    // (only looked for, should the host do something of it).
    assert!(!p.typed(&[0x7f], Some(&screen("$ ")), t + ms(2000)).1);
    assert_eq!(p.overlay(), None);
    assert!(!p.typed(b"k", Some(&screen("$ ")), t + ms(2100)).1, "and what is typed after it is unseen until it proves true");
}

#[test]
fn what_isnt_echoed_is_never_shown() {
    // A password: letters typed after Enter, none echoed.
    let (mut p, t) = warm();
    let (enter, _) = p.typed(b"\r", Some(&screen("$ sudo true")), t);
    let prompt = screen("[sudo] password: ");
    p.answered(enter, t + ms(200));
    p.frame(Some(&prompt), t + ms(250));
    let mut last = enter;
    for (i, letter) in b"hunter2".iter().enumerate() {
        let (seq, changed) = p.typed(&[*letter], Some(&prompt), t + ms(400 + 100 * i as u64));
        assert!(!changed && p.overlay().is_none(), "nothing of a password is drawn");
        last = seq;
    }
    // The host takes them and echoes nothing: given up, unseen as they were.
    p.answered(last, t + ms(1400));
    let (next, changed) = p.tick(t + ms(1500));
    assert_eq!((next, changed), (Some(t + ms(1400) + ADRIFT), false));
    assert_eq!(p.tick(t + ms(3000)), (None, false));
    assert!(!p.waits());
    // And typed on, after them: still nothing.
    assert!(!p.typed(b"!", Some(&prompt), t + ms(3100)).1);
}

#[test]
fn the_first_letter_after_enter_waits_and_the_rest_dont() {
    let (mut p, t) = warm();
    let on = screen("$ x");
    p.typed(b"l", Some(&on), t);
    p.typed(b"s", Some(&on), t);
    assert_eq!(shown(&p), Some(("ls@3".into(), 5)));
    // Enter, and typing goes straight on: what was shown stays, what is new is unseen.
    p.typed(b"\r", Some(&on), t + ms(100));
    assert!(!p.typed(b"p", Some(&on), t + ms(200)).1);
    assert_eq!(shown(&p), Some(("ls@3".into(), 5)));
    // The host: ls echoed, then run, a new prompt, and the p after it.
    assert!(p.frame(Some(&screen("$ xls")), t + ms(400)));
    assert_eq!(p.overlay(), None);
    p.frame(Some(&screen("$ ")), t + ms(500));
    assert!(p.waits(), "the p is still looked for");
    // (Its echo a moment before the answer for it: known when that comes.)
    p.frame(Some(&screen("$ p")), t + ms(520));
    assert!(p.waits(), "a key the host hasn't answered for isn't held to be the one seen");
    taken(&mut p, t + ms(525));
    assert!(!p.waits(), "seen typed between two frames: the gap is over");
    // From then they show at once.
    assert!(p.typed(b"w", Some(&screen("$ p")), t + ms(600)).1);
    assert!(p.typed(b"d", Some(&screen("$ p")), t + ms(650)).1);
    assert_eq!(shown(&p), Some(("wd@3".into(), 5)));
}

#[test]
fn keys_behind_a_gap_are_found_though_some_went_unseen() {
    let (mut p, t) = warm();
    let on = screen("$ x");
    p.typed(b"\r", Some(&on), t);
    type_all(&mut p, &[b"m", b"a", b"k", b"e"], &on, t + ms(50));
    // The new prompt and the first letter come in one frame (nothing
    // to know them by), the next two in another: those are known.
    taken(&mut p, t + ms(390));
    p.frame(Some(&screen("$ m")), t + ms(400));
    assert!(p.waits());
    assert_eq!(p.overlay(), None);
    assert!(p.frame(Some(&screen("$ mak")), t + ms(450)), "the e, typed in the same epoch, is shown from now");
    assert_eq!(shown(&p), Some(("e@5".into(), 6)));
    p.frame(Some(&screen("$ make")), t + ms(500));
    assert!(!p.waits());
}

#[test]
fn a_prompt_that_appears_by_itself_is_proven_against_anew() {
    // (Gemini's review, 2026-11-22: a first cut looked at no frame
    // while nothing waited, and kept its trust through this.)
    let (mut p, t) = warm();
    // A script answered "x" goes on to ask for a secret, with no Enter of ours.
    let asked = screen("Password: ");
    p.frame(Some(&asked), t);
    for (i, letter) in b"hunter2".iter().enumerate() {
        assert!(!p.typed(&[*letter], Some(&asked), t + ms(100 * i as u64)).1);
        assert_eq!(p.overlay(), None);
    }
    // The same when the frame that shows it isn't seen before the key
    // is typed (typed() is told the screen too).
    let (mut p, t) = warm();
    assert!(!p.typed(b"h", Some(&asked), t).1);
    // And a change of the cursor's shape (a mode: vi's, an editor's).
    let (mut p, t) = warm();
    let mut block = screen("$ x");
    block.shape = 2;
    p.frame(Some(&block), t);
    assert!(!p.typed(b"j", Some(&block), t).1);
}

#[test]
fn a_mask_proves_nothing_and_taints_its_line() {
    // Behind an Enter, at a prompt that draws a star for each letter:
    // a star typed is no proof, whatever comes back.
    let (mut p, t) = warm();
    p.typed(b"\r", Some(&screen("$ x")), t);
    p.frame(Some(&screen("PIN: ")), t + ms(300));
    p.typed(b"*", Some(&screen("PIN: ")), t + ms(400));
    taken(&mut p, t + ms(690));
    p.frame(Some(&screen("PIN: *")), t + ms(700));
    for (i, letter) in b"1234".iter().enumerate() {
        assert!(!p.typed(&[*letter], Some(&screen("PIN: *")), t + ms(800 + 50 * i as u64)).1);
        assert_eq!(p.overlay(), None);
    }
    // At a place that was trusted (the same prompt, used for a secret
    // this time): the first letter is drawn, the host shows a star for
    // it, and from then nothing on that line is.
    let (mut p, t) = warm();
    assert!(p.typed(b"a", Some(&screen("$ x")), t).1);
    p.answered(2, t + ms(200));
    p.frame(Some(&screen("$ x*")), t + ms(300));
    assert_eq!(p.overlay(), None, "not drawn while in doubt");
    p.tick(t + ms(300) + SETTLE);
    for (i, letter) in b"bcd".iter().enumerate() {
        let host = format!("$ x*{}", "*".repeat(i));
        p.frame(Some(&screen(&host)), t + ms(400 + 100 * i as u64));
        assert!(!p.typed(&[*letter], Some(&screen(&host)), t + ms(410 + 100 * i as u64)).1, "{host}");
    }
    // A letter that does come back as typed doesn't lift it either…
    p.frame(Some(&screen("$ x***")), t + ms(900));
    p.typed(b"e", Some(&screen("$ x***")), t + ms(910));
    taken(&mut p, t + ms(1190));
    p.frame(Some(&screen("$ x***e")), t + ms(1200));
    assert!(!p.typed(b"f", Some(&screen("$ x***e")), t + ms(1300)).1);
    // …only the caret's leaving the line.
    p.tick(t + ms(5000));
    p.typed(b"\r", Some(&screen("$ x***e")), t + ms(5000));
    p.frame(Some(&screen("$ ")), t + ms(5300));
    p.typed(b"l", Some(&screen("$ ")), t + ms(5400));
    taken(&mut p, t + ms(5690));
    p.frame(Some(&screen("$ l")), t + ms(5700));
    assert!(p.typed(b"s", Some(&screen("$ l")), t + ms(5800)).1);
}

#[test]
fn keys_after_one_that_isnt_predicted_arent_placed_on_the_screen_before_it() {
    // (Gemini's review: a first cut put the c after "ab", as if Tab did
    // nothing, and took Tab's own "c" for its echo.)
    let (mut p, t) = warm();
    let on = screen("$ x");
    type_all(&mut p, &[b"a", b"b"], &on, t);
    p.typed(b"\t", Some(&on), t + ms(50));
    assert!(!p.typed(b"c", Some(&on), t + ms(100)).1);
    assert_eq!(shown(&p), Some(("ab@3".into(), 5)), "only what was typed before Tab");
    // Tab completes to "xabc": not the c's echo.
    p.frame(Some(&screen("$ xabc")), t + ms(400));
    p.tick(t + ms(400) + SETTLE);
    assert_eq!(p.overlay(), None);
    assert!(p.waits(), "the c is still looked for");
    assert!(!p.typed(b"d", Some(&screen("$ xabc")), t + ms(450)).1, "and nothing is shown on its word");
    // Its echo: seen as typed, and the d shown on its word.
    taken(&mut p, t + ms(490));
    p.frame(Some(&screen("$ xabcc")), t + ms(500));
    assert_eq!(shown(&p), Some(("d@7".into(), 8)));
}

#[test]
fn a_wrong_prediction_is_dropped_and_doubted_after() {
    let (mut p, t) = warm();
    p.typed(b"q", Some(&screen("$ x")), t);
    assert!(p.overlay().is_some());
    // The program doesn't echo it: the cursor goes elsewhere (a pager took the key).
    let elsewhere = screen(":");
    assert!(p.frame(Some(&elsewhere), t + ms(300)), "not drawn from the moment it is in doubt");
    assert_eq!(p.overlay(), None);
    assert_eq!(p.tick(t + ms(310)).0, Some(t + ms(300) + SETTLE));
    p.tick(t + ms(300) + SETTLE);
    // What is typed next is unseen until one comes true.
    assert!(!p.typed(b"j", Some(&elsewhere), t + ms(400)).1);
    assert_eq!(p.overlay(), None);
}

#[test]
fn a_piece_of_a_redraw_is_not_a_contradiction() {
    let (mut p, t) = warm();
    p.typed(b"a", Some(&screen("$ x")), t);
    // The program is in the middle of drawing: no caret to be found,
    // then the line half drawn. Nothing is drawn ahead meanwhile…
    p.frame(None, t + ms(300));
    assert_eq!(p.overlay(), None);
    p.frame(Some(&screen("$")), t + ms(305));
    // …and the frame that follows is the echo: believed, and the next
    // letter is shown at once (nothing was given up).
    p.frame(Some(&screen("$ xa")), t + ms(310));
    assert!(!p.waits());
    assert_eq!(p.tick(t + ms(400)), (None, false));
    assert!(p.typed(b"b", Some(&screen("$ xa")), t + ms(500)).1);
}

#[test]
fn a_caret_the_program_draws_is_typed_at() {
    // An input box as Claude Code draws it: the cursor hidden, the
    // caret a cell in reverse video, an edge at the right.
    let boxed = |text: &str| {
        let mut s = drawn(text, text.chars().count() as u16);
        s.cells[WIDTH - 2] = Cell::of('│');
        s
    };
    let mut p = Predictor::new(true);
    let t = Instant::now();
    p.frame(Some(&boxed("│ > ")), t);
    p.typed(b"h", Some(&boxed("│ > ")), t);
    p.frame(Some(&boxed("│ > h")), t + ms(300));
    assert!(p.typed(b"e", Some(&boxed("│ > h")), t + ms(400)).1);
    assert!(p.typed(b"y", Some(&boxed("│ > h")), t + ms(450)).1);
    // The letters where the caret was, and the caret drawn after them.
    assert_eq!(shown(&p), Some(("ey|@5".into(), 7)));
    assert!(p.overlay().unwrap().drawn);
    // The box moves up as a menu opens under it: found by what it holds.
    let mut moved = boxed("│ > he");
    moved.row = 2;
    p.frame(Some(&moved), t + ms(700));
    assert_eq!(shown(&p), Some(("y|@6".into(), 7)));
    assert_eq!(p.overlay().unwrap().row, 2);
    // Never up to the box's own edge: the program wraps before it.
    let full = boxed(&format!("│ > {}", "w".repeat(WIDTH - 8)));
    p.frame(Some(&full), t + ms(900));
    assert!(!p.typed(b"z", Some(&full), t + ms(1000)).1);
    assert_eq!(p.overlay(), None);
    // The caret moved back with the arrow, a letter put in: the text
    // after it is pushed along, the caret drawn on the letter it is at.
    let (mut p, t) = (Predictor::new(true), Instant::now());
    p.frame(Some(&boxed("│ > ")), t);
    p.typed(b"a", Some(&boxed("│ > ")), t);
    p.frame(Some(&boxed("│ > a")), t + ms(300));
    type_all(&mut p, &[b"b", b"\x1b[D", b"z"], &boxed("│ > a"), t + ms(400));
    let o = p.overlay().unwrap();
    assert_eq!((o.col, o.caret), (5, 6), "{o:?}");
    assert_eq!(o.cells, vec![('z', Like::Typed), ('b', Like::Caret)]);
}

#[test]
fn what_is_right_of_the_caret_is_pushed_written_over_or_left_alone() {
    // A placeholder (dim) is written over: all of it goes with a
    // letter that isn't its own first…
    let (mut p, t) = warm();
    let hint = dimmed(at("$ xType here", 3), 3, 12);
    p.frame(Some(&hint), t);
    p.typed(b"h", Some(&hint), t);
    p.frame(Some(&screen("$ xh")), t + ms(300));
    let (mut p, t) = warm();
    p.frame(Some(&hint), t);
    // (the caret didn't move and the text before it is the same: the same epoch)
    assert!(p.typed(b"h", Some(&hint), t).1);
    assert_eq!(shown(&p), Some(("h________@3".into(), 4)));
    // …and stays where the letter is its first (a suggestion taken up).
    let (mut p, t) = warm();
    p.frame(Some(&hint), t);
    assert!(p.typed(b"T", Some(&hint), t).1);
    assert_eq!(shown(&p), Some(("Type_here@3".into(), 4)));
    assert_eq!(p.overlay().unwrap().cells[1], ('y', Like::Cell(4)));
    // The line's own text is pushed along, and what is past a gap (a
    // right prompt) is never touched.
    let (mut p, t) = warm();
    let mut on = screen("$ x");
    for (i, c) in "12:00".chars().enumerate() {
        on.cells[30 + i] = Cell::of(c);
    }
    p.frame(Some(&on), t);
    type_all(&mut p, &[b"a", b"b", b"\x1b[D", b"\x1b[D", b"z"], &on, t);
    let o = p.overlay().unwrap();
    assert_eq!((o.col, o.caret), (3, 4));
    assert_eq!(o.cells, vec![('z', Like::Typed), ('a', Like::Typed), ('b', Like::Typed)]);
    // The right arrow goes back across what the left one crossed, and
    // no further (what else is there may be a suggestion, which it takes).
    assert!(p.typed(b"\x1b[C", Some(&on), t).1);
    assert!(p.typed(b"\x1b[C", Some(&on), t).1);
    assert_eq!(p.overlay().unwrap().caret, 6);
    assert!(!p.typed(b"\x1b[C", Some(&on), t).1);
}

#[test]
fn only_what_can_be_known_is_predicted() {
    let (mut p, t) = warm();
    // The last columns: what comes after them is the terminal's wrap.
    let full = screen(&"x".repeat(WIDTH - 2));
    p.frame(Some(&full), t);
    assert!(!p.typed(b"z", Some(&full), t).1);
    assert!(p.overlay().is_none() && p.placed() == 0, "not placed: only looked for");
    // More than a few keys at once is a paste; a control key and an
    // escape aren't predicted; a letter that takes no cell isn't.
    for bytes in [&b"a long line pasted in"[..], b"\x03", b"\x1b[A", "\u{301}".as_bytes()] {
        let (mut p, t) = warm();
        assert!(!p.typed(bytes, Some(&screen("$ x")), t).1);
        assert!(!p.waits(), "{bytes:?}");
    }
    // No caret to type at: nothing.
    let (mut p, t) = warm();
    p.frame(None, t);
    assert!(!p.typed(b"a", None, t).1);
    assert!(p.overlay().is_none() && p.placed() == 0);
    // Letters of other alphabets are, one cell wide or two.
    let (mut p, t) = warm();
    assert!(p.typed("é".as_bytes(), Some(&screen("$ x")), t).1);
    assert!(p.typed("日".as_bytes(), Some(&screen("$ x")), t).1);
    assert_eq!(p.overlay().unwrap().caret, 6);
    assert!(p.typed(&[0x7f], Some(&screen("$ x")), t).1, "and a wide one is taken back whole");
    assert_eq!(p.overlay().unwrap().caret, 4);
}

#[test]
fn a_quick_echo_is_left_to_the_host() {
    // On this computer the echo is back within a frame or two: nothing is drawn ahead of it.
    let mut p = Predictor::new(true);
    let t = Instant::now();
    p.frame(Some(&screen("$ ")), t);
    p.typed(b"a", Some(&screen("$ ")), t);
    p.frame(Some(&screen("$ a")), t + ms(8));
    assert!(!p.typed(b"b", Some(&screen("$ a")), t + ms(100)).1);
    assert_eq!(p.overlay(), None);
    assert!(p.waits(), "it is still followed, in case the link slows");
    // Turned off: nothing waits at all.
    p.set_on(false);
    assert!(!p.waits());
    assert!(!p.typed(b"c", Some(&screen("$ ab")), t).1);
    assert!(!p.waits());
}

#[test]
fn a_secret_the_host_says_it_reads_is_never_drawn() {
    let (mut p, t) = warm();
    p.typed(b"a", Some(&screen("$ x")), t);
    assert!(p.overlay().is_some());
    assert!(p.set_secret(true), "what was drawn goes");
    assert!(!p.typed(b"b", Some(&screen("$ x")), t).1);
    assert_eq!(p.overlay(), None);
}

#[test]
fn a_link_that_stops_keeps_what_was_typed_in_sight() {
    let (mut p, t) = warm();
    for (i, letter) in b"make".iter().enumerate() {
        p.typed(&[*letter], Some(&screen("$ x")), t + ms(100 * i as u64));
    }
    // Nothing answered, nothing echoed, for ten seconds: still drawn
    // (nothing says it is wrong), and how long is known.
    gone(&mut p, t);
    assert_eq!(p.tick(t + ms(10_000)), (None, false));
    assert_eq!(shown(&p), Some(("make@3".into(), 7)));
    // (Said from two seconds, and anew at each second.)
    assert_eq!(p.stalled(t + ms(1_900)), None);
    assert_eq!(p.stall_changes(t + ms(1_900)), Some(t + ms(2_000)));
    assert_eq!(p.stalled(t + ms(10_300)), Some(ms(10_300)));
    assert_eq!(p.stall_changes(t + ms(10_300)), Some(t + ms(11_000)));
    assert_eq!(p.stall_changes(t + ms(75_000)), Some(t + ms(120_000)), "by the minute from one minute");
    // The link is back: all of it at once.
    p.answered(5, t + ms(11_000));
    assert!(p.frame(Some(&screen("$ xmake")), t + ms(11_000)));
    assert_eq!(p.overlay(), None);
    assert_eq!((p.stalled(t + ms(11_000)), p.stall_changes(t + ms(11_000))), (None, None));
}

#[test]
fn what_the_host_took_and_didnt_show_is_given_up() {
    let (mut p, t) = warm();
    let (seq, _) = p.typed(b"a", Some(&screen("$ x")), t);
    assert!(p.overlay().is_some());
    p.answered(seq, t + ms(200));
    let wait = NO_ECHO.max(ms(600));
    assert_eq!(p.tick(t + ms(250)), (Some(t + ms(200) + wait), false));
    assert_eq!(p.tick(t + ms(200) + wait), (None, true));
    assert_eq!(p.overlay(), None);
    // And what is typed after is unseen until one comes true.
    assert!(!p.typed(b"b", Some(&screen("$ x")), t + ms(2000)).1);
}

// ---- What the played sessions found (`played.rs`), each as it was ----

#[test]
fn a_letter_typed_and_taken_back_proves_nothing() {
    // At a prompt that echoes nothing, behind an Enter: a letter and a
    // Backspace leave the screen as it was, which is also what a host
    // that echoes nothing shows. Not an echo.
    let (mut p, t) = warm();
    p.typed(b"\r", Some(&screen("$ x")), t);
    let asked = screen("Password: ");
    p.frame(Some(&asked), t + ms(300));
    type_all(&mut p, &[b"h", b"\x7f"], &asked, t + ms(400));
    gone(&mut p, t + ms(400));
    taken(&mut p, t + ms(700));
    p.frame(Some(&asked), t + ms(710));
    p.frame(Some(&asked), t + ms(720));
    for (i, letter) in b"unter2".iter().enumerate() {
        assert!(!p.typed(&[*letter], Some(&asked), t + ms(800 + 60 * i as u64)).1);
        assert_eq!(p.overlay(), None);
    }
}

#[test]
fn a_password_s_letters_arent_taken_for_the_next_prompt_s_echo() {
    // A password typed (never echoed), Enter, and the first letter at
    // the prompt that follows is one the password had too: its echo is
    // its own, not the password's letter's, and nothing of the password
    // is drawn after it.
    let (mut p, t) = warm();
    p.typed(b"\r", Some(&screen("$ x")), t);
    let asked = screen("Password: ");
    p.frame(Some(&asked), t + ms(300));
    type_all(&mut p, &[b"d", b"e", b"j", b"a"], &asked, t + ms(400));
    gone(&mut p, t + ms(400));
    taken(&mut p, t + ms(650));
    p.typed(b"\r", Some(&asked), t + ms(700));
    p.typed(b"d", Some(&asked), t + ms(750));
    gone(&mut p, t + ms(750));
    p.frame(Some(&screen("$ ")), t + ms(1000));
    taken(&mut p, t + ms(1040));
    p.frame(Some(&screen("$ d")), t + ms(1050));
    assert_eq!(p.overlay(), None, "nothing of the password after it");
    // (The d was the prompt's own: what is typed next is shown.)
    assert!(p.typed(b"f", Some(&screen("$ d")), t + ms(1100)).1);
    assert_eq!(shown(&p), Some(("f@3".into(), 4)));
}

#[test]
fn of_the_same_letter_twice_the_one_seen_is_known_or_none_is() {
    // "ll" behind an Enter, the first echoed in the frame the prompt
    // came in: the second's frame shows the second (the first stands
    // before it), and a third "l" isn't drawn.
    let (mut p, t) = warm();
    p.typed(b"\r", Some(&screen("$ x")), t);
    type_all(&mut p, &[b"l", b"l"], &screen("$ x"), t + ms(50));
    gone(&mut p, t + ms(50));
    taken(&mut p, t + ms(380));
    p.frame(Some(&screen("$ l")), t + ms(400));
    p.frame(Some(&screen("$ ll")), t + ms(410));
    assert!(!p.waits(), "both are over");
    assert_eq!(p.overlay(), None);
    // "s", Tab, "s": the first frame's "s" is the first one's.
    let (mut p, t) = warm();
    p.typed(b"\r", Some(&screen("$ x")), t);
    p.frame(Some(&screen("$ ")), t + ms(300));
    type_all(&mut p, &[b" ", b"s", b"\t", b"s", b"t"], &screen("$ "), t + ms(400));
    gone(&mut p, t + ms(400));
    taken(&mut p, t + ms(700));
    p.frame(Some(&screen("$  ")), t + ms(705));
    p.frame(Some(&screen("$  s")), t + ms(710));
    assert!(p.waits(), "the second s and the t are still to come");
    p.frame(Some(&screen("$  s_")), t + ms(715));
    p.frame(Some(&screen("$  s_st")), t + ms(720));
    assert!(!p.waits());
    assert!(p.typed(b"y", Some(&screen("$  s_st")), t + ms(800)).1);
    assert_eq!(shown(&p), Some(("y@7".into(), 8)));
}

// What the played sessions found with each key sent as it is typed
// (Olo's helper on the host: several writes on their way at once).

#[test]
fn an_echo_isnt_back_before_the_write_before_it_is_answered() {
    let (mut p, t) = warm();
    // (A round trip is timed: 0.3 s.)
    let (a, _) = p.typed(b"a", Some(&screen("$ x")), t);
    p.went(a, t);
    p.answered(a, t + ms(300));
    p.frame(Some(&screen("$ xa")), t + ms(305));
    // Enter, "*", "q", Enter, "q": each sent as typed.
    let (on, t) = (screen("$ xa"), t + ms(1000));
    let seqs: Vec<u64> = [&b"\r"[..], b"*", b"q", b"\r", b"q"]
        .iter()
        .enumerate()
        .map(|(i, key)| {
            let at = t + ms(10 * i as u64);
            let (seq, _) = p.typed(key, Some(&on), at);
            p.went(seq, at);
            seq
        })
        .collect();
    // The host takes the first three; the prompt comes, the echoes of
    // "*" and "q" don't (the link holds them), and they are no longer
    // looked for.
    p.answered(seqs[0], t + ms(600));
    p.frame(Some(&screen("$ ")), t + ms(605));
    p.answered(seqs[2], t + ms(620));
    p.tick(t + ms(900));
    // They come late. The second Enter isn't answered, so the "q" on
    // the screen can't be the one typed after it, though by the clock
    // alone that one could be back: nothing is held to be seen, and
    // nothing typed now is drawn on the strength of it.
    p.frame(Some(&screen("$ *")), t + ms(1000));
    p.frame(Some(&screen("$ *q")), t + ms(1010));
    assert!(p.waits(), "the second q is still looked for");
    assert!(!p.typed(b"z", Some(&screen("$ *q")), t + ms(1020)).1);
    assert_eq!(p.overlay(), None);
}

#[test]
fn a_caret_gone_right_over_a_letter_isnt_that_letter_typed() {
    // A program that draws its caret, the caret on the "b" of "ab".
    // Something not predicted, then a right arrow and a "b".
    let mut p = Predictor::new(true);
    let t = Instant::now();
    let base = drawn("> ab", 3);
    p.frame(Some(&base), t);
    type_all(&mut p, &[b"\t", b"\x1b[C", b"b"], &base, t + ms(10));
    gone(&mut p, t + ms(10));
    taken(&mut p, t + ms(300));
    // The caret goes right, over the "b" that was there: the text
    // before it is what a "b" typed would leave before it too, but the
    // caret stands on nothing, where a "b" typed would have pushed the
    // old one under it.
    p.frame(Some(&drawn("> ab", 4)), t + ms(310));
    assert!(p.waits(), "the b typed is still to come");
    p.frame(Some(&drawn("> abb", 5)), t + ms(320));
    assert!(!p.waits(), "and now it has");
}
