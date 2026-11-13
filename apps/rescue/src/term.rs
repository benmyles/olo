//! Rescue's terminal: a shell in a pseudo-terminal of this process's
//! own, alacritty_terminal's emulator read and written on its own thread
//! (`EventLoop`), drawn on the cell grid here. The shell and its screen
//! are a [`Session`], the program's (made once, whichever display Rescue
//! shows on); a [`Terminal`] is the view of it in a window. Deliberately apart from
//! Terminal (apps/terminal): no olo-termd, no saved windows or settings,
//! nothing of the person's session to depend on. The shell is bash
//! without its startup files (`bash --noprofile --norc`: a broken
//! profile or shell can't take it away), or `/bin/sh`.
//!
//! What it does: the screen and its colours (the look's terminal
//! colours, a program's own 256 and true colours), bold, italic,
//! underlines, wide characters, the cursor; the keys (xterm's), ⌃⇧C and
//! ⌃⇧V (⌘C and ⌘V: Rescue is a terminal to the keymap), ⇧Insert; a
//! selection by the pointer; the scrollback by the wheel and ⇧Page Up
//! and Down; a program's mouse and alternate screen scrolling as
//! arrows.

use std::{
    borrow::Cow,
    cell::Cell as StdCell,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    event_loop::{EventLoop, EventLoopSender, Msg},
    grid::Scroll,
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    sync::FairMutex,
    term::{Config, Term as Emulator, TermMode, cell::Flags, color::Colors, test::TermSize},
    tty,
    vte::ansi::{Color, CursorShape, NamedColor, Rgb},
};
use olo_kit::{
    Bounds, ClipboardItem, Cx, Element, FocusHandle, FontStyle, FontWeight, Hsla, InteractiveElement as _, KeyDownEvent, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Painter,
    ParentElement as _, Pixels, Proxy, ScrollWheelEvent, Styled as _, TextRun, View, clipboard, div, point, px, size, style, theme, view_canvas,
};

/// The text's size, points.
const TEXT_SIZE: f32 = 12.0;
/// Between the well's edge and the text.
const PAD: f32 = 4.0;

/// The shell printed (at most one in flight: `Listener::pending`).
pub struct Wake;
/// The shell ended.
pub struct Exited;

#[derive(Clone)]
pub struct Listener {
    proxy: Proxy,
    pending: Arc<AtomicBool>,
    writer: Arc<std::sync::OnceLock<EventLoopSender>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Wakeup if !self.pending.swap(true, Ordering::AcqRel) => self.proxy.send(Wake),
            // Answers to the program (cursor position, colours asked).
            Event::PtyWrite(s) => {
                if let Some(w) = self.writer.get() {
                    let _ = w.send(Msg::Input(Cow::Owned(s.into_bytes())));
                }
            }
            Event::ChildExit(_) | Event::Exit => self.proxy.send(Exited),
            _ => {}
        }
    }
}

/// The cell grid: a cell's size and the baseline in it, on device pixels.
#[derive(Clone, Copy, PartialEq)]
struct Metrics {
    cell: (f32, f32),
    baseline: f32,
    scale: f32,
}

impl Metrics {
    fn new(g: &Painter) -> Metrics {
        let scale = g.scale_factor();
        let ts = g.text_system();
        let id = ts.resolve_font(&style::mono_font(FontWeight::NORMAL));
        let fs = px(TEXT_SIZE);
        let advance = ts.advance(id, fs, 'M').map_or(TEXT_SIZE * 0.6, |a| f32::from(a.width));
        let ascent = f32::from(ts.ascent(id, fs));
        let descent = f32::from(ts.descent(id, fs)).abs();
        let snap = |v: f32| (v * scale).ceil() / scale;
        let cell = (snap(advance), snap(ascent + descent));
        let baseline = (((cell.1 - ascent - descent) / 2.0 + ascent) * scale).round() / scale;
        Metrics { cell, baseline, scale }
    }

    fn grid(&self, w: f32, h: f32) -> (usize, usize) {
        ((((w - 2.0 * PAD) / self.cell.0).floor() as usize).max(2), (((h - 2.0 * PAD) / self.cell.1).floor() as usize).max(1))
    }
}

/// Where the text was last drawn, and its cells.
type Drawn = (Bounds<Pixels>, Metrics);

/// The shell and its screen: the program's, shown by whichever window
/// has Rescue (its [`Terminal`]).
#[derive(Clone)]
pub struct Session {
    term: Arc<FairMutex<Emulator<Listener>>>,
    tx: Option<EventLoopSender>,
    pending: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
    /// What went wrong starting the shell.
    pub error: Option<String>,
}

fn shell() -> tty::Shell {
    let bash = ["/usr/bin/bash", "/bin/bash"].into_iter().find(|p| std::path::Path::new(p).exists());
    match bash {
        Some(b) => tty::Shell::new(b.into(), vec!["--noprofile".into(), "--norc".into(), "-i".into()]),
        None => tty::Shell::new("/bin/sh".into(), vec!["-i".into()]),
    }
}

impl Session {
    /// Starts the shell; `proxy` hears [`Wake`] and [`Exited`].
    pub fn new(proxy: Proxy) -> Session {
        let (cols, rows) = (100, 30);
        let pending = Arc::new(AtomicBool::new(false));
        let writer = Arc::new(std::sync::OnceLock::new());
        let listener = Listener { proxy, pending: pending.clone(), writer: writer.clone() };
        let config = Config { scrolling_history: 10_000, ..Config::default() };
        let term = Arc::new(FairMutex::new(Emulator::new(config, &TermSize::new(cols, rows), listener.clone())));
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let mut env = std::collections::HashMap::new();
        env.insert("TERM".to_owned(), "xterm-256color".to_owned());
        env.insert("COLORTERM".to_owned(), "truecolor".to_owned());
        env.insert("PS1".to_owned(), "rescue \\w \\$ ".to_owned());
        let options = tty::Options { shell: Some(shell()), working_directory: home, drain_on_exit: false, env };
        let size = WindowSize { num_lines: rows as u16, num_cols: cols as u16, cell_width: 8, cell_height: 16 };
        let (tx, error) = match tty::new(&options, size, 0).and_then(|pty| EventLoop::new(term.clone(), listener, pty, false, false)) {
            Ok(event_loop) => {
                let tx = event_loop.channel();
                let _ = writer.set(tx.clone());
                // (Its thread inherits the process's priority, Rescue's.)
                let _ = event_loop.spawn();
                (Some(tx), None)
            }
            Err(e) => {
                tracing::warn!("Rescue's shell: {e}");
                (None, Some(format!("The shell didn't start: {e}")))
            }
        };
        Session { term, tx, pending, exited: Arc::default(), error }
    }

    /// The shell has ended.
    pub fn set_exited(&self) {
        self.exited.store(true, Ordering::Release);
    }

    pub fn exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    /// What the shell printed is about to be shown: it may say so again.
    pub fn shown(&self) {
        self.pending.store(false, Ordering::Release);
    }

    fn write(&self, bytes: Vec<u8>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Input(Cow::Owned(bytes)));
        }
    }

    /// Typed: back to the bottom, the selection gone.
    fn typed(&self, bytes: Vec<u8>) {
        {
            let mut t = self.term.lock();
            t.scroll_display(Scroll::Bottom);
            t.selection = None;
        }
        self.write(bytes);
    }

    /// `text` typed as a paste (in brackets, if the program asked).
    fn pasted(&self, text: String) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let bytes = if bracketed { format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")) } else { text };
        self.typed(bytes.into_bytes());
    }
}

/// The terminal in a window: the session's screen drawn, and what's
/// typed and pointed at there.
pub struct Terminal {
    session: Session,
    pub focus: FocusHandle,
    /// The grid's columns and rows, as the shell was last told.
    grid: StdCell<(usize, usize)>,
    /// Where the text starts and a cell's size, as last drawn.
    drawn: StdCell<Option<Drawn>>,
    selecting: bool,
}

impl Terminal {
    pub fn new(session: Session) -> Terminal {
        // (What the shell has printed so far is in the first drawing: it
        // says so again from here. Printed before there was a window to
        // tell, its one word of it went nowhere, and it never said
        // another.)
        session.shown();
        // (No size the shell was told yet: the first drawing tells it this
        // window's.)
        Terminal { session, focus: FocusHandle::new(), grid: StdCell::new((0, 0)), drawn: StdCell::new(None), selecting: false }
    }

    /// The shell printed: drawn again.
    pub fn woke(&mut self, cx: &mut Cx<Self>) {
        self.session.shown();
        cx.invalidate();
    }

    fn copy(&self) {
        if let Some(text) = self.session.term.lock().selection_to_string().filter(|t| !t.is_empty()) {
            clipboard::write(ClipboardItem::new_string(text));
        }
    }

    /// What's on the clipboard typed, once it has come (the program that
    /// has it is asked: nothing here waits).
    fn paste(&self) {
        let session = self.session.clone();
        clipboard::paste_item(move |item| {
            if let Some(text) = item.and_then(|c| c.text()) {
                session.pasted(text);
            }
        });
    }

    fn key(&mut self, k: &Keystroke, cx: &mut Cx<Self>) -> bool {
        let m = &k.modifiers;
        // ⌃⇧C, ⌃⇧V (what the keymap makes ⌘C and ⌘V for a terminal), ⇧Insert.
        if m.control && m.shift && !m.alt {
            match k.key.as_str() {
                "c" | "C" => {
                    self.copy();
                    return true;
                }
                "v" | "V" => {
                    self.paste();
                    return true;
                }
                _ => {}
            }
        }
        if m.shift && !m.control && !m.alt {
            let lines = self.grid.get().1 as i32 - 1;
            match k.key.as_str() {
                "insert" => {
                    self.paste();
                    return true;
                }
                "pageup" => {
                    self.session.term.lock().scroll_display(Scroll::Delta(lines));
                    cx.invalidate();
                    return true;
                }
                "pagedown" => {
                    self.session.term.lock().scroll_display(Scroll::Delta(-lines));
                    cx.invalidate();
                    return true;
                }
                _ => {}
            }
        }
        if m.platform {
            return false;
        }
        let mode = *self.session.term.lock().mode();
        if let Some(bytes) = key_bytes(k, mode) {
            self.session.typed(bytes);
            return true;
        }
        if let Some(text) = k.key_char.as_ref().filter(|t| !t.is_empty() && !m.control) {
            self.session.typed(text.clone().into_bytes());
            return true;
        }
        false
    }

    fn cell_at(&self, at: olo_kit::Point<Pixels>) -> Option<(Point, Side)> {
        let (b, m) = self.drawn.get()?;
        let (cols, rows) = self.grid.get();
        let x = (f32::from(at.x - b.left()) - PAD).max(0.0);
        let y = (f32::from(at.y - b.top()) - PAD).max(0.0);
        let col = ((x / m.cell.0) as usize).min(cols.saturating_sub(1));
        let row = ((y / m.cell.1) as usize).min(rows.saturating_sub(1));
        let side = if x - col as f32 * m.cell.0 < m.cell.0 / 2.0 { Side::Left } else { Side::Right };
        let offset = self.session.term.lock().grid().display_offset() as i32;
        Some((Point::new(Line(row as i32 - offset), Column(col)), side))
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, cx: &mut Cx<Self>) {
        self.focus.focus(cx);
        if e.button != MouseButton::Left {
            return;
        }
        let Some((p, side)) = self.cell_at(e.position) else { return };
        let kind = match e.click_count {
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.session.term.lock().selection = Some(Selection::new(kind, p, side));
        self.selecting = true;
        cx.invalidate();
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, cx: &mut Cx<Self>) {
        if !self.selecting || e.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some((p, side)) = self.cell_at(e.position) else { return };
        if let Some(s) = self.session.term.lock().selection.as_mut() {
            s.update(p, side);
        }
        cx.invalidate();
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, cx: &mut Cx<Self>) {
        self.selecting = false;
        // A click alone selects nothing.
        {
            let mut t = self.session.term.lock();
            if t.selection.as_ref().is_some_and(|s| s.is_empty()) {
                t.selection = None;
            }
        }
        cx.invalidate();
    }

    fn scroll(&mut self, e: &ScrollWheelEvent, cx: &mut Cx<Self>) {
        let Some((_, m)) = self.drawn.get() else { return };
        let dy = f32::from(e.delta.pixel_delta(px(m.cell.1)).y);
        let lines = (dy / m.cell.1).round() as i32;
        if lines == 0 {
            return;
        }
        let mode = *self.session.term.lock().mode();
        // A full-screen program (less, an editor) scrolls by its arrows.
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let arrow: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.session.write(arrow.repeat(lines.unsigned_abs() as usize));
            return;
        }
        self.session.term.lock().scroll_display(Scroll::Delta(lines));
        cx.invalidate();
    }
}

/// An emulator colour as the look's terminal colours have it.
struct Look {
    fg: Rgb,
    bg: Rgb,
    cursor: Rgb,
    selection: Rgb,
    ansi: [Rgb; 16],
}

impl Look {
    fn new(t: &olo_theme::Terminal) -> Look {
        let rgb = |[r, g, b, _]: olo_theme::Rgba| Rgb { r, g, b };
        Look { fg: rgb(t.foreground), bg: rgb(t.background), cursor: rgb(t.cursor), selection: rgb(t.selection), ansi: t.ansi.map(rgb) }
    }

    fn indexed(&self, i: u8) -> Rgb {
        match i {
            0..16 => self.ansi[i as usize],
            16..232 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
                Rgb { r: level(i / 36), g: level((i / 6) % 6), b: level(i % 6) }
            }
            _ => {
                let v = 8 + (i - 232) * 10;
                Rgb { r: v, g: v, b: v }
            }
        }
    }

    fn resolve(&self, c: Color, colors: &Colors) -> Rgb {
        match c {
            Color::Spec(rgb) => rgb,
            Color::Indexed(i) => colors[i as usize].unwrap_or_else(|| self.indexed(i)),
            Color::Named(n) => {
                if let Some(set) = colors[n] {
                    return set;
                }
                let i = n as usize;
                match n {
                    NamedColor::Foreground | NamedColor::BrightForeground => self.fg,
                    NamedColor::DimForeground => dim(self.fg),
                    NamedColor::Background => self.bg,
                    NamedColor::Cursor => self.cursor,
                    _ if i < 16 => self.ansi[i],
                    // The dim ones, after the cursor's.
                    _ => dim(self.ansi[(i - NamedColor::DimBlack as usize).min(7)]),
                }
            }
        }
    }
}

fn dim(c: Rgb) -> Rgb {
    Rgb { r: (c.r as u16 * 2 / 3) as u8, g: (c.g as u16 * 2 / 3) as u8, b: (c.b as u16 * 2 / 3) as u8 }
}

fn hsla(c: Rgb) -> Hsla {
    style::c([c.r, c.g, c.b, 255])
}

/// A row's text in one style, from column `col`.
struct Run {
    col: usize,
    text: String,
    color: Hsla,
    weight: FontWeight,
    italic: bool,
    underline: bool,
    /// Its characters each two cells wide.
    wide: bool,
}

impl View for Terminal {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let look = Look::new(&p.terminal);
        let focused = self.focus.is_focused(cx);
        let bg = hsla(look.bg);
        let screen = view_canvas(move |t_view: &Terminal, g, well| {
            let term = &t_view.session.term;
            let m = Metrics::new(g);
            // The screen in a sunken well, inside its two lines.
            g.paint_quad(olo_kit::fill(style::snap(g, well), bg));
            style::sunken(g, well, &p, focused);
            let line = 2.0 / m.scale;
            let b = Bounds::from_corners(point(well.left() + px(line), well.top() + px(line)), point(well.right() - px(line), well.bottom() - px(line)));
            t_view.drawn.set(Some((b, m)));
            // The well's size in cells: the shell told when it changes.
            let (cols, rows) = m.grid(f32::from(b.size.width), f32::from(b.size.height));
            // (Not a well too small for a prompt: two columns told to the
            // shell wrap its prompt, and it doesn't come back as it was.
            // Its window, a point square until app_server sizes it, isn't
            // laid out or drawn before: `awaits_size`.)
            if t_view.grid.get() != (cols, rows) && cols >= 20 && rows >= 4 {
                t_view.grid.set((cols, rows));
                term.lock().resize(TermSize::new(cols, rows));
                if let Some(tx) = &t_view.session.tx {
                    let cell = ((m.cell.0 * m.scale).round() as u16, (m.cell.1 * m.scale).round() as u16);
                    let _ = tx.send(Msg::Resize(WindowSize { num_lines: rows as u16, num_cols: cols as u16, cell_width: cell.0, cell_height: cell.1 }));
                }
            }
            let t = term.lock();
            let content = t.renderable_content();
            let offset = content.display_offset as i32;
            let (ox, oy) = (f32::from(b.left()) + PAD, f32::from(b.top()) + PAD);
            let (cw, ch) = m.cell;
            let snap = |v: f32| px((v * m.scale).round() / m.scale);
            let at = |line: i32, col: usize| (snap(ox + col as f32 * cw), snap(oy + (line + offset) as f32 * ch));
            let selection = content.selection;
            let cursor = content.cursor;
            let colors = content.colors;
            let mut runs: Vec<(i32, Run)> = Vec::new();
            for item in content.display_iter {
                let (pt, cell) = (item.point, item.cell);
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                let (mut fg, mut cbg) = (look.resolve(cell.fg, colors), look.resolve(cell.bg, colors));
                if cell.flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut cbg);
                }
                if cell.flags.contains(Flags::DIM) {
                    fg = dim(fg);
                }
                let wide = cell.flags.contains(Flags::WIDE_CHAR);
                let w = if wide { 2.0 * cw } else { cw };
                let selected = selection.is_some_and(|s| s.contains(pt));
                let fill = if selected { Some(look.selection) } else { (cbg != look.bg).then_some(cbg) };
                if let Some(c) = fill {
                    let (x, y) = at(pt.line.0, pt.column.0);
                    g.paint_quad(olo_kit::fill(Bounds { origin: point(x, y), size: size(px(w), px(ch)) }, hsla(c)));
                }
                if cell.c == ' ' || cell.c == '\t' || cell.flags.contains(Flags::HIDDEN) {
                    // (A space ends nothing: it's in the run, if one's going.)
                    if let Some((line, r)) = runs.last_mut()
                        && *line == pt.line.0
                        && !r.wide
                        && r.col + r.text.chars().count() == pt.column.0
                    {
                        r.text.push(' ');
                    }
                    continue;
                }
                let color = hsla(fg);
                let weight = if cell.flags.contains(Flags::BOLD) { FontWeight::BOLD } else { FontWeight::NORMAL };
                let italic = cell.flags.contains(Flags::ITALIC);
                let underline = cell.flags.intersects(Flags::ALL_UNDERLINES);
                let mut text = cell.c.to_string();
                if let Some(z) = cell.zerowidth() {
                    text.extend(z);
                }
                match runs.last_mut() {
                    Some((line, r))
                        if *line == pt.line.0
                            && !wide
                            && !r.wide
                            && r.color == color
                            && r.weight == weight
                            && r.italic == italic
                            && r.underline == underline
                            && r.col + r.text.chars().count() == pt.column.0 =>
                    {
                        r.text.push_str(&text);
                    }
                    _ => runs.push((pt.line.0, Run { col: pt.column.0, text, color, weight, italic, underline, wide })),
                }
            }
            for (line, r) in runs {
                let text = r.text.trim_end().to_owned();
                if text.is_empty() {
                    continue;
                }
                let mut font = style::mono_font(r.weight);
                if r.italic {
                    font.style = FontStyle::Italic;
                }
                let underline = r.underline.then(|| olo_kit::UnderlineStyle { thickness: px(1.0), color: Some(r.color), wavy: false });
                let run = TextRun { len: text.len(), font, color: r.color, background_color: None, underline, strikethrough: None, size: None };
                let w = if r.wide { 2.0 * cw } else { cw };
                let shaped = g.text_system().shape_line(text.into(), px(TEXT_SIZE), &[run], Some(px(w)));
                let (x, y) = at(line, r.col);
                let _ = shaped.paint(point(x, y + px(m.baseline) - shaped.ascent), shaped.ascent + shaped.descent, g);
            }
            // The cursor: a block while the keyboard is here, an
            // outline while it isn't.
            if cursor.shape != CursorShape::Hidden && offset == 0 {
                let (x, y) = at(cursor.point.line.0, cursor.point.column.0);
                let r = Bounds { origin: point(x, y), size: size(px(cw), px(ch)) };
                let c = hsla(look.cursor);
                let line = px(1.0 / m.scale).max(px(1.0));
                match (focused, cursor.shape) {
                    (true, CursorShape::Beam) => g.paint_quad(olo_kit::fill(Bounds { origin: r.origin, size: size(line * 2.0, r.size.height) }, c)),
                    (true, CursorShape::Underline) => g.paint_quad(olo_kit::fill(Bounds { origin: point(r.left(), r.bottom() - line * 2.0), size: size(r.size.width, line * 2.0) }, c)),
                    (true, _) => {
                        g.paint_quad(olo_kit::fill(r, c));
                        let under = t.grid()[cursor.point].c;
                        if under != ' ' {
                            let run = TextRun {
                                len: under.len_utf8(),
                                font: style::mono_font(FontWeight::NORMAL),
                                color: hsla(look.bg),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                                size: None,
                            };
                            let shaped = g.text_system().shape_line(under.to_string().into(), px(TEXT_SIZE), &[run], Some(px(cw)));
                            let _ = shaped.paint(point(x, y + px(m.baseline) - shaped.ascent), shaped.ascent + shaped.descent, g);
                        }
                    }
                    (false, _) => g.paint_quad(olo_kit::outline(r, c, olo_kit::BorderStyle::Solid)),
                }
            }
            g.agent(olo_kit::agent::Node { role: "Terminal", name: Some("Terminal".into()), bounds: well, enabled: true, ..Default::default() });
        })
        .size_full();
        div()
            .size_full()
            .track_focus(&self.focus)
            .focusable()
            // (Its cursor shows whether the keyboard is here.)
            .on_focus_changed(cx.listener(|_: &mut Self, _, cx| cx.invalidate()))
            // Every key is the shell's while the keyboard is here: Tab
            // too (the window's own Tab, to the next control, isn't
            // reached).
            .on_key_down(cx.listener(|t: &mut Self, e: &KeyDownEvent, cx| {
                if t.key(&e.keystroke, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .child(screen)
    }
}

/// xterm's modifier parameter: 1 + shift + 2·alt + 4·ctrl.
fn modifier_param(k: &Keystroke) -> u8 {
    1 + k.modifiers.shift as u8 + 2 * k.modifiers.alt as u8 + 4 * k.modifiers.control as u8
}

/// The bytes for a key, or None when it's left to the text it types (as
/// Terminal's `view::key_bytes`, without the kitty protocol, which
/// Rescue's emulator doesn't offer).
fn key_bytes(k: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let mp = modifier_param(k);
    let (ctrl, alt, shift) = (k.modifiers.control, k.modifiers.alt, k.modifiers.shift);
    let csi = |letter: char| -> Vec<u8> {
        if mp > 1 {
            format!("\x1b[1;{mp}{letter}").into_bytes()
        } else if mode.contains(TermMode::APP_CURSOR) {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> { if mp > 1 { format!("\x1b[{n};{mp}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() } };
    let ss3 = |letter: char| -> Vec<u8> { if mp > 1 { format!("\x1b[1;{mp}{letter}").into_bytes() } else { format!("\x1bO{letter}").into_bytes() } };
    let esc = |mut b: Vec<u8>| {
        if alt {
            b.insert(0, 0x1b);
        }
        b
    };
    let name = k.key.as_str();
    Some(match name {
        "up" => csi('A'),
        "down" => csi('B'),
        "right" if alt && !ctrl && !shift => b"\x1bf".to_vec(),
        "left" if alt && !ctrl && !shift => b"\x1bb".to_vec(),
        "right" => csi('C'),
        "left" => csi('D'),
        "home" => csi('H'),
        "end" => csi('F'),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "f1" => ss3('P'),
        "f2" => ss3('Q'),
        "f3" => ss3('R'),
        "f4" => ss3('S'),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "enter" => esc(vec![b'\r']),
        "tab" if shift => b"\x1b[Z".to_vec(),
        "tab" => esc(vec![b'\t']),
        "backspace" if ctrl => esc(vec![0x08]),
        "backspace" => esc(vec![0x7f]),
        "escape" => esc(vec![0x1b]),
        _ if ctrl => esc(vec![ctrl_byte(name, shift)?]),
        _ if alt => {
            let c = if name == "space" { ' ' } else { single(name)? };
            esc(if shift { c.to_ascii_uppercase() } else { c }.to_string().into_bytes())
        }
        _ => return None,
    })
}

fn single(name: &str) -> Option<char> {
    let mut chars = name.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

/// Ctrl+key as a C0 control character (as xterm's).
fn ctrl_byte(name: &str, shift: bool) -> Option<u8> {
    if name == "space" {
        return Some(0);
    }
    let c = single(name)?;
    if c.is_ascii_alphabetic() {
        return Some(c.to_ascii_lowercase() as u8 & 0x1f);
    }
    let c = if shift { olo_kit::keys::shifted(c).unwrap_or(c) } else { c };
    Some(match c {
        '2' | '@' => 0,
        '3' | '[' | '{' => 0x1b,
        '4' | '\\' | '|' => 0x1c,
        '5' | ']' | '}' => 0x1d,
        '6' | '^' | '~' => 0x1e,
        '7' | '-' | '_' | '/' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> Keystroke {
        Keystroke::parse(s).unwrap()
    }

    #[test]
    fn keys_as_xterm_sends_them() {
        let none = TermMode::empty();
        assert_eq!(key_bytes(&key("up"), none), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_bytes(&key("up"), TermMode::APP_CURSOR), Some(b"\x1bOA".to_vec()));
        assert_eq!(key_bytes(&key("ctrl-c"), none), Some(vec![3]));
        assert_eq!(key_bytes(&key("ctrl-shift-up"), none), Some(b"\x1b[1;6A".to_vec()));
        assert_eq!(key_bytes(&key("backspace"), none), Some(vec![0x7f]));
        assert_eq!(key_bytes(&key("enter"), none), Some(vec![b'\r']));
        assert_eq!(key_bytes(&key("alt-b"), none), Some(b"\x1bb".to_vec()));
        // A letter alone is the text's.
        assert_eq!(key_bytes(&key("a"), none), None);
    }
}
