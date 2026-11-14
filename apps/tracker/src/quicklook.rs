//! Quick Look: Space on a selected item previews it at once, in a window
//! that follows the selection (arrow keys step through the folder, Space
//! or Escape closes). The previews come from `olo-preview`; this adds the
//! footer (what it is, rotate, pop out to Viewer, Open) and the keys.
//! Rotating a picture (⌘L / ⌘R, or the footer's buttons) saves it at
//! once, as Quick Look does on the Mac; anything more is Viewer's job.

use std::path::PathBuf;

use olo_kit::{
    Bounds, Child, ClickEvent, Cx, Element, FocusHandle, FontWeight, InteractiveElement as _, IntoElement as _, KeyDownEvent, Menu, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Painter,
    ParentElement as _, PathBuilder, Pixels, Point, Styled as _, View, WindowCx, WindowOptions, canvas, div, icons, point, px, size,
    style::{self, Align},
    theme,
    widgets::Button,
};
use olo_kit_base::metrics as m;
pub use olo_preview::edit::EditEvent;
use olo_preview::{
    Body, Item,
    edit::{self, Editor, Op},
    kit::{PreviewEvent, PreviewView},
};

use crate::{
    ToHub,
    fs::{self, Entry},
    items, to_hub,
};

/// The bar along the bottom: what the item is, pop out, and Open.
pub const FOOTER: f32 = m::CONTROL_HEIGHT + 2.0 * m::TIGHT + 2.0;

/// Quick Look's window: a panel over the window whose selection it shows,
/// never one of Tracker's tabs, so it's as big as what it shows wants.
pub fn window_options(title: String) -> WindowOptions {
    WindowOptions::new(title, 480.0, 360.0).min_size(320.0, 200.0).child().dialog()
}

fn item(e: &Entry) -> Item {
    Item { path: e.path.clone(), name: e.name.clone(), is_dir: e.is_dir, size: e.size, modified: e.modified, kind: e.kind, icon: items::icon_of(e).to_string() }
}

/// What Quick Look's window is told.
pub enum QuickMsg {
    /// Another item (of window `.0`'s), in the same window.
    Show(u64, Entry, u64),
    /// The picture's editor says something (of the picture at a path).
    Edited(PathBuf, EditEvent),
    /// A drag out of it is over.
    DragOver,
    Close,
}

pub struct QuickLook {
    /// The folder or Find window (or the desktop) whose selection shows.
    pub source: u64,
    pub entry: Entry,
    pub preview: Child<PreviewView>,
    /// "Open with …": who opens the entry (looked up once per item).
    opener: Option<String>,
    /// Rotating the picture (made at the first turn).
    editor: Option<Editor>,
    /// The entry is a picture that can be written back.
    editable: bool,
    /// The last rotation couldn't be saved.
    failed: Option<String>,
    /// What the picture is ("JPEG image"), kept for the note as it turns.
    what: String,
    /// Its preview has come (or failed): the window's size follows it.
    pub shown: bool,
    awake: bool,
    press: Option<Point<Pixels>>,
    /// Has the keyboard (its keys: Space, Escape, the arrows).
    focus: FocusHandle,
}

impl QuickLook {
    /// Start loading `entry` (`generation` tells its updates apart). The
    /// window shows once its preview has come and it has its size, or
    /// after a moment with a spinner if that is slow.
    pub fn new(source: u64, entry: Entry, generation: u64, cx: &mut Cx<Self>) -> QuickLook {
        cx.hold_commit(olo_preview::SLOW);
        let it = item(&entry);
        let preview = cx.new_view(|cx| PreviewView::new(it, generation, false, cx));
        cx.subscribe(&preview, |q: &mut QuickLook, e: &PreviewEvent, cx| {
            if *e == PreviewEvent::Shown {
                q.shown = true;
                q.fit(cx);
            }
            cx.invalidate();
        });
        // Dragging the picture out where it doesn't pan: the file.
        let me = cx.view_ref();
        preview.update(cx, |p, cx| {
            p.image_view().update(cx, move |v, _| {
                v.on_drag_out(move |cx| {
                    me.update(cx, |q, cx| {
                        let d = q.drag(point(px(16.0), px(16.0)));
                        items::start(cx, u64::MAX, d, || QuickMsg::DragOver);
                    });
                })
            });
        });
        let opener = fs::opener_name(&entry);
        let editable = !entry.is_dir && edit::editable(&entry.path);
        let focus = FocusHandle::new();
        focus.focus(cx);
        QuickLook { source, entry, preview, opener, editor: None, editable, failed: None, what: String::new(), shown: false, awake: false, press: None, focus }
    }

    /// Show another item in the same window (what's shown stays until it
    /// comes, so stepping through a folder doesn't flash).
    pub fn retarget(&mut self, source: u64, entry: Entry, generation: u64, cx: &mut Cx<Self>) {
        self.source = source;
        let it = item(&entry);
        self.preview.update(cx, |p, cx| p.retarget(it, generation, cx));
        self.opener = fs::opener_name(&entry);
        // A rotation still being written finishes on its own thread.
        self.editor = None;
        self.failed = None;
        self.editable = !entry.is_dir && edit::editable(&entry.path);
        cx.set_window_title(entry.name.clone());
        self.entry = entry;
        cx.invalidate();
    }

    /// The window takes the size that suits what is shown, within its
    /// screen, and shows (if it was waiting to).
    fn fit(&mut self, cx: &mut Cx<Self>) {
        if !cx.is_fullscreen() {
            let (w, h) = self.ideal_size(screen(cx), cx);
            cx.resize_window(size(px(w as f32), px(h as f32)));
        }
        cx.release_commit();
    }

    /// Turn the picture a quarter and save it in place; the result comes
    /// back to [`QuickLook::edited`].
    pub fn rotate(&mut self, right: bool, cx: &mut Cx<Self>) {
        if !self.editable {
            return;
        }
        if self.editor.is_none() {
            let Some(Body::Image { full, .. }) = self.preview.read(cx).body() else { return };
            let full = *full;
            self.what = self.preview.read(cx).note().split(", ").next().unwrap_or_default().to_owned();
            let (to, path) = (cx.messenger(), self.entry.path.clone());
            self.editor = Some(Editor::new(self.entry.path.clone(), (full[0] as u32, full[1] as u32), move |ev| {
                to.send(QuickMsg::Edited(path.clone(), ev));
            }));
        }
        let e = self.editor.as_mut().expect("made");
        e.push(if right { Op::RotateRight } else { Op::RotateLeft });
        e.save(self.entry.path.clone());
        self.failed = None;
        cx.invalidate();
    }

    /// The editor reports `event` (for the picture at `path`); true when
    /// it was this one's and the picture turned (the window may want
    /// another size).
    pub fn edited(&mut self, path: &std::path::Path, event: EditEvent, cx: &mut Cx<Self>) -> bool {
        let Some(e) = self.editor.as_mut().filter(|e| e.path == path) else { return false };
        e.handle(&event);
        let turned = match event {
            EditEvent::Shown { image, full, .. } => {
                let note = format!("{}, {} × {}", self.what, full[0] as u32, full[1] as u32);
                self.preview.update(cx, |p, cx| p.show_edited(image, full, note, cx));
                true
            }
            EditEvent::Saved(_) => false,
            EditEvent::Failed(why) => {
                self.failed = Some(why);
                false
            }
        };
        cx.invalidate();
        turned
    }

    /// A rotation is being worked out or written.
    fn busy(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.busy || e.saving)
    }

    /// The window size that suits what is shown, within the screen.
    pub fn ideal_size(&self, screen: Option<(u32, u32)>, cx: &WindowCx) -> (u32, u32) {
        let p = self.preview.read(cx);
        let (w, h) = p.ideal_size(screen, FOOTER);
        // Room for the note beside the rotate buttons.
        let min = if self.editable && p.is_image() { 460 } else { 0 };
        (w.max(min.min(screen.map_or(u32::MAX, |s| s.0))), h)
    }

    /// Dragging the item out, the pointer `hotspot` into its icon.
    fn drag(&self, hotspot: Point<Pixels>) -> items::Drag {
        let e = &self.entry;
        items::Drag { paths: vec![e.path.clone()], icon: items::icon_of(e), size: 32.0, hotspot, ask: false, name: e.name.clone() }
    }

    /// Opens what it shows (a folder's window, a file by what opens its
    /// kind), and goes.
    fn open(&mut self, cx: &mut Cx<Self>) {
        if self.entry.is_dir {
            to_hub(ToHub::Open { path: self.entry.path.clone(), from: None });
        } else {
            fs::open(&self.entry);
        }
        cx.close_window();
    }

    fn key_down(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) {
        let k = &e.keystroke;
        // A video or a sound: `>` a speed faster, `<` slower (its
        // transport's speed button lists them).
        if let Some(by) = olo_preview::kit::speed_key(k)
            && self.preview.read(cx).playback_rate().is_some()
        {
            self.preview.update(cx, |p, cx| p.step_playback_rate(by, cx));
            cx.stop_propagation();
            return;
        }
        let command = k.modifiers.control || k.modifiers.platform;
        let arrows = self.preview.read(cx).arrows_free();
        let media = self.preview.read(cx).has_media_time();
        let plain = !command && !k.modifiers.alt && !k.modifiers.shift;
        match k.key.as_str() {
            "space" | "escape" if plain => cx.close_window(),
            "enter" if plain => self.open(cx),
            // A video or a sound: ← and → move its time by 10 s, ↑ and ↓ go
            // to the next file.
            "left" if plain && media => {
                self.preview.update(cx, |p, cx| p.skip(-PreviewView::SKIP_SECS, cx));
            }
            "right" if plain && media => {
                self.preview.update(cx, |p, cx| p.skip(PreviewView::SKIP_SECS, cx));
            }
            "left" | "up" if plain && arrows => to_hub(ToHub::LookStep(-1)),
            "right" | "down" if plain && arrows => to_hub(ToHub::LookStep(1)),
            "f" if plain => cx.toggle_fullscreen(),
            "l" | "r" if command => self.rotate(k.key == "r", cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl View for QuickLook {
    type Message = QuickMsg;

    fn message(&mut self, message: QuickMsg, cx: &mut Cx<Self>) {
        match message {
            QuickMsg::Show(source, entry, generation) => self.retarget(source, entry, generation, cx),
            QuickMsg::Edited(path, event) => {
                if self.edited(&path, event, cx) {
                    self.fit(cx);
                }
            }
            QuickMsg::DragOver => self.press = None,
            QuickMsg::Close => cx.close_window(),
        }
    }

    fn window_closing(&mut self, cx: &mut Cx<Self>) {
        self.preview.update(cx, |p, cx| p.stop(cx));
        to_hub(ToHub::LookClosed);
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        self.focus.focus_if_none(cx);
        // The screen stays awake while a video plays.
        let playing = self.preview.read(cx).playing_video();
        if playing != self.awake {
            self.awake = playing;
            cx.set_keep_awake(playing);
        }
        let p = theme::palette();
        let label = match &self.opener {
            Some(app) => format!("Open with {app}"),
            None => "Open".into(),
        };
        // Pop out (to the Viewer) is a square button showing a window with
        // an arrow; rotate left and right are for pictures that can be
        // saved.
        let pw = m::CONTROL_HEIGHT + 4.0;
        let pop = !self.entry.is_dir && self.opener.as_deref() != Some("Viewer");
        let rotate = self.editable && self.preview.read(cx).is_image();
        let note = match &self.failed {
            Some(why) => why.clone(),
            None if self.busy() => "Rotating…".into(),
            None => self.preview.read(cx).note(),
        };
        let icon = items::icon_of(&self.entry);
        let info = canvas(move |g, b| {
            let p = theme::palette();
            // Level with the note's capitals, on the device grid.
            let r = style::snap_centred(g, point(b.left() + px(8.0), b.center().y), 16.0, 16.0);
            icons::paint(g, r, icon);
            let x = r.right() + px(2.0 * m::TIGHT);
            let at = style::label_fit(g, x, b.center().y, Align::Left, note.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p), (b.right() - x).max(px(0.0)));
            olo_kit::agent::record(g, olo_kit::agent::Node::label(note.clone(), at));
        })
        .size_full();
        // The icon is the item: drag it out, as a document's proxy icon.
        let grab = div()
            .absolute()
            .left_0()
            .top_0()
            .bottom_0()
            .w(px(20.0))
            .on_mouse_down(MouseButton::Left, cx.listener(|q: &mut QuickLook, ev: &MouseDownEvent, _| q.press = Some(ev.position)))
            .on_window_mouse_up(cx.listener(|q: &mut QuickLook, _: &MouseUpEvent, _| q.press = None))
            .on_window_mouse_move(cx.listener(|q: &mut QuickLook, ev: &MouseMoveEvent, cx| {
                let Some(at) = q.press.filter(|_| ev.pressed_button == Some(MouseButton::Left)) else { return };
                if (ev.position - at).magnitude() < 3.0 {
                    return;
                }
                q.press = None;
                let d = q.drag(point(px(8.0), px(8.0)));
                items::start(cx, u64::MAX, d, || QuickMsg::DragOver);
            }));
        let mut buttons = div().flex().flex_row().items_center().gap(px(m::TIGHT));
        if rotate {
            for (right, tip) in [(false, "Rotate Left (⌘L)"), (true, "Rotate Right (⌘R)")] {
                buttons = buttons.child(olo_kit::tip::on(
                    div()
                        .relative()
                        .child(Button::new("").min_width(pw).on_click(cx.listener(move |q: &mut QuickLook, _: &ClickEvent, cx| q.rotate(right, cx))))
                        .child(canvas(move |g, b| rotate_glyph(g, b, right, &theme::palette())).absolute().inset_0()),
                    if right { "rotate-right" } else { "rotate-left" },
                    tip,
                ));
            }
            buttons = buttons.child(div().w(px(m::SPACING - m::TIGHT)));
        }
        if pop {
            buttons = buttons.child(olo_kit::tip::on(
                div()
                    .relative()
                    .child(Button::new("").min_width(pw).on_click(cx.listener(|q: &mut QuickLook, _: &ClickEvent, cx| {
                        fs::open_in_viewer(&q.entry.path);
                        cx.close_window();
                    })))
                    .child(canvas(|g, b| pop_out_glyph(g, b, &theme::palette())).absolute().inset_0()),
                "pop-out",
                "Open in Viewer",
            ));
        }
        let label_w = style::text_width(label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL));
        // Its secondary button: Open With ▸ the kind's apps.
        let open_button = div()
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|q: &mut QuickLook, ev: &MouseDownEvent, cx| {
                    let Some(menu) = crate::folder::open_with_menu(std::slice::from_ref(&q.entry)) else { return };
                    let me = cx.view_ref();
                    olo_kit::menu::context_menu(cx, Menu::new("").submenu(menu), ev.position, move |cmd, cx| {
                        let cmd = cmd.to_owned();
                        me.update(cx, move |q, cx| {
                            crate::open_with(&cmd, std::slice::from_ref(&q.entry));
                            cx.close_window();
                        });
                    });
                }),
            )
            .child(Button::new(label).min_width((f32::from(label_w) + 28.0).max(80.0)).on_click(cx.listener(|q: &mut QuickLook, _: &ClickEvent, cx| q.open(cx))));
        buttons = buttons.child(div().w(px(m::SPACING - m::TIGHT))).child(open_button);
        let edge = canvas(move |g, b| {
            style::fill_rect(g, b, style::c(p.panel_background));
            style::hline(g, b.left(), b.right(), b.top(), style::shade(&p, p.panel_background, 1.2));
            let lit = b.top() + style::hairline(g);
            style::hline(g, b.left(), b.right(), lit, style::c(p.shine));
        })
        .absolute()
        .inset_0();
        let footer = div().h(px(FOOTER)).w_full().relative().child(edge).child(
            div()
                .absolute()
                .left(px(m::SPACING))
                .right(px(m::SPACING))
                .top(px(m::TIGHT + 2.0))
                .bottom(px(m::TIGHT))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().flex_1().h_full().relative().child(info).child(grab))
                .child(buttons),
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|q: &mut QuickLook, e: &KeyDownEvent, cx| q.key_down(e, cx)))
            .child(div().flex_1().min_h_0().relative().child(&self.preview))
            .child(footer)
            .into_any_element()
    }
}

/// A three-quarter circle with an arrowhead, turning left or right.
fn rotate_glyph(window: &mut Painter, r: Bounds<Pixels>, right: bool, p: &olo_theme::Palette) {
    let ink = style::c(p.control_text);
    let o = point(r.center().x, r.center().y + px(0.5));
    let flip = if right { 1.0 } else { -1.0 };
    // From the top, round the far side and back up to the near one.
    let mut path = PathBuilder::stroke(style::hairline(window));
    for k in 0..=24 {
        let a = -std::f32::consts::FRAC_PI_2 - flip * (k as f32 / 24.0) * 1.5 * std::f32::consts::PI;
        let pt = point(o.x + px(5.0 * a.cos()), o.y + px(5.0 * a.sin()));
        if k == 0 { path.move_to(pt) } else { path.line_to(pt) }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, ink);
    }
    // The arrowhead at the top, pointing the way it turns.
    let tip = point(o.x + px(flip * 2.5), o.y - px(5.0));
    let mut head = PathBuilder::fill();
    head.move_to(point(tip.x - px(flip * 3.0), tip.y - px(3.0)));
    head.line_to(tip);
    head.line_to(point(tip.x - px(flip * 3.0), tip.y + px(3.0)));
    head.close();
    if let Ok(head) = head.build() {
        window.paint_path(head, ink);
    }
}

/// A small window frame with an arrow leaving its top-right corner, its
/// lines on whole device pixels.
fn pop_out_glyph(window: &mut Painter, r: Bounds<Pixels>, p: &olo_theme::Palette) {
    let ink = style::c(p.control_text);
    let o = point(style::snap_coord(window, r.center().x - px(6.0)), style::snap_coord(window, r.center().y - px(5.0)));
    let h = |window: &mut Painter, x0: f32, x1: f32, y: f32| style::hline(window, o.x + px(x0), o.x + px(x1), o.y + px(y), ink);
    let v = |window: &mut Painter, x: f32, y0: f32, y1: f32| style::vline(window, o.x + px(x), o.y + px(y0), o.y + px(y1), ink);
    // The frame, open at the top-right where the arrow leaves.
    v(window, 0.0, 3.0, 12.0);
    h(window, 0.0, 9.0, 11.0);
    v(window, 8.0, 7.0, 12.0);
    h(window, 0.0, 5.0, 3.0);
    // The arrow.
    h(window, 7.0, 12.0, 0.0);
    v(window, 11.0, 0.0, 5.0);
    let w = style::hairline(window);
    window.paint_segment(point(o.x + px(4.0) + w / 2.0, o.y + px(7.0) + w / 2.0), point(o.x + px(11.0) + w / 2.0, o.y + w / 2.0), w, false, ink);
}

/// The screen a window is on (or the smallest, before it's on one).
pub fn screen(cx: &WindowCx) -> Option<(u32, u32)> {
    let size = |b: &Bounds<Pixels>| (f32::from(b.size.width) as u32, f32::from(b.size.height) as u32);
    cx.display_bounds().map(|b| size(&b)).or_else(|| olo_kit::displays().iter().map(size).min_by_key(|a| a.0 * a.1))
}
