//! Quick Look: Space on a selected item previews it at once, in a window
//! that follows the selection (arrow keys step through the folder, Space
//! or Escape closes). The previews come from `olo-preview`; this adds the
//! footer (what it is, rotate, pop out to Viewer, Open) and the keys.
//! Rotating a picture (⌘L / ⌘R, or the footer's buttons) saves it at
//! once, as Quick Look does on the Mac; anything more is Viewer's job.

use olo_kit_base::metrics as m;
pub use olo_preview::edit::EditEvent;
use olo_preview::{
    Body, Item,
    edit::{self, Editor, Op},
    ui::{PreviewEvent, PreviewView},
};
use olo_ui::{
    WindowOptions,
    gpui::{
        self, App, AppContext as _, Bounds, Context, Entity, EntityId, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseMoveEvent, ParentElement as _, PathBuilder,
        Pixels, Point, Render, StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window, canvas, div, point, px, size,
    },
    icons,
    style::{self, Align},
    theme,
    widgets::{Button, tooltip},
};

use crate::{
    Tracker,
    fs::{self, Entry},
    items,
};

/// The bar along the bottom: what the item is, pop out, and Open.
pub const FOOTER: f32 = m::CONTROL_HEIGHT + 2.0 * m::TIGHT + 2.0;

/// Quick Look's window: a panel over the window whose selection it shows,
/// never one of Tracker's tabs, so it's as big as what it shows wants.
pub fn window_options(title: String, (w, h): (u32, u32)) -> WindowOptions {
    WindowOptions::new(title, w as f32, h as f32).min_size(320.0, 200.0).child().dialog()
}

/// What the person asked of Quick Look (the Tracker does it).
pub enum Action {
    Close,
    Open,
    /// Keep looking at it in its own Viewer window.
    PopOut,
    /// Preview the previous (-1) or next (+1) item in the folder.
    Step(isize),
    /// Open With ▸ (its command: an app, Always, Other…).
    OpenWith(String),
}

fn item(e: &Entry) -> Item {
    Item { path: e.path.clone(), name: e.name.clone(), is_dir: e.is_dir, size: e.size, modified: e.modified, kind: e.kind, icon: items::icon_of(e).to_string() }
}

pub struct QuickLook {
    pub app: WeakEntity<Tracker>,
    /// The folder or Find window (or the desktop) whose selection shows.
    pub source: EntityId,
    pub entry: Entry,
    pub preview: Entity<PreviewView>,
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
    /// The window it shows in, once open.
    pub win: Option<gpui::WindowHandle<olo_ui::Frame>>,
    /// Its preview has come (or failed): the window's size follows it.
    pub shown: bool,
    awake: bool,
    press: Option<Point<Pixels>>,
    /// Has the keyboard (its keys: Space, Escape, the arrows), given it
    /// as its window opens.
    focus: gpui::FocusHandle,
    focused: bool,
    _sub: Subscription,
}

impl QuickLook {
    /// Start loading `entry` (`generation` tells its updates apart).
    pub fn new(app: WeakEntity<Tracker>, source: EntityId, entry: Entry, generation: u64, cx: &mut Context<Self>) -> QuickLook {
        let preview = cx.new(|cx| PreviewView::new(item(&entry), generation, false, cx));
        let sub = cx.subscribe(&preview, |q, _, e: &PreviewEvent, cx| {
            if *e == PreviewEvent::Shown {
                q.shown = true;
                let (app, me) = (q.app.clone(), cx.entity());
                cx.defer(move |cx| {
                    if let Some(app) = app.upgrade() {
                        app.update(cx, |t, cx| t.quick_look_shown(&me, cx));
                    }
                });
            }
            cx.notify();
        });
        // Dragging the picture out where it doesn't pan: the file.
        let me = cx.entity();
        preview.read(cx).image_view().clone().update(cx, |v, _| {
            v.on_drag_out(move |window, cx| {
                let d = me.read(cx).drag(point(px(16.0), px(16.0)));
                items::start(window, cx, me.entity_id(), d);
            })
        });
        let opener = fs::opener_name(&entry);
        let editable = !entry.is_dir && edit::editable(&entry.path);
        QuickLook {
            app,
            source,
            entry,
            preview,
            opener,
            editor: None,
            editable,
            failed: None,
            what: String::new(),
            win: None,
            shown: false,
            awake: false,
            press: None,
            focus: cx.focus_handle(),
            focused: false,
            _sub: sub,
        }
    }

    /// Show another item in the same window (what's shown stays until it
    /// comes, so stepping through a folder doesn't flash).
    pub fn retarget(&mut self, source: EntityId, entry: Entry, generation: u64, cx: &mut Context<Self>) {
        self.source = source;
        let it = item(&entry);
        self.preview.update(cx, |p, cx| p.retarget(it, generation, cx));
        self.opener = fs::opener_name(&entry);
        // A rotation still being written finishes on its own thread.
        self.editor = None;
        self.failed = None;
        self.editable = !entry.is_dir && edit::editable(&entry.path);
        self.entry = entry;
        cx.notify();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.preview.update(cx, |p, _| p.stop());
    }

    /// Turn the picture a quarter and save it in place; the result comes
    /// back to [`QuickLook::edited`].
    pub fn rotate(&mut self, right: bool, cx: &mut Context<Self>) {
        if !self.editable {
            return;
        }
        if self.editor.is_none() {
            let Some(Body::Image { full, .. }) = self.preview.read(cx).body() else { return };
            let full = *full;
            self.what = self.preview.read(cx).note().split(", ").next().unwrap_or_default().to_owned();
            let (proxy, path) = (olo_ui::proxy(cx), self.entry.path.clone());
            self.editor = Some(Editor::new(self.entry.path.clone(), (full[0] as u32, full[1] as u32), move |ev| proxy.send(crate::Msg::Edited(path.clone(), ev))));
        }
        let e = self.editor.as_mut().expect("made");
        e.push(if right { Op::RotateRight } else { Op::RotateLeft });
        e.save(self.entry.path.clone());
        self.failed = None;
        cx.notify();
    }

    /// The editor reports `event` (for the picture at `path`); true when
    /// it was this one's and the picture turned (the window may want
    /// another size).
    pub fn edited(&mut self, path: &std::path::Path, event: EditEvent, cx: &mut Context<Self>) -> bool {
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
        cx.notify();
        turned
    }

    /// A rotation is being worked out or written.
    fn busy(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.busy || e.saving)
    }

    /// The window size that suits what is shown, within the screen.
    pub fn ideal_size(&self, screen: Option<(u32, u32)>, cx: &App) -> (u32, u32) {
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

    fn act(&self, a: Action, cx: &mut Context<Self>) {
        let app = self.app.clone();
        cx.defer(move |cx| {
            if let Some(app) = app.upgrade() {
                app.update(cx, |t, cx| t.quick_look_action(a, cx));
            }
        });
    }

    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let command = k.modifiers.control || k.modifiers.platform;
        let arrows = self.preview.read(cx).arrows_free();
        let media = self.preview.read(cx).has_media_time();
        let plain = !command && !k.modifiers.alt && !k.modifiers.shift;
        match k.key.as_str() {
            "space" | "escape" if plain => self.act(Action::Close, cx),
            "enter" if plain => self.act(Action::Open, cx),
            // A video or a sound: ← and → move its time by 10 s, ↑ and ↓ go
            // to the next file.
            "left" if plain && media => {
                self.preview.update(cx, |p, cx| p.skip(-olo_preview::ui::PreviewView::SKIP_SECS, cx));
            }
            "right" if plain && media => {
                self.preview.update(cx, |p, cx| p.skip(olo_preview::ui::PreviewView::SKIP_SECS, cx));
            }
            "left" | "up" if plain && arrows => self.act(Action::Step(-1), cx),
            "right" | "down" if plain && arrows => self.act(Action::Step(1), cx),
            "f" if plain => window.toggle_fullscreen(),
            "l" | "r" if command => self.rotate(k.key == "r", cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for QuickLook {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus);
        }
        // The screen stays awake while a video plays.
        let playing = self.preview.read(cx).playing_video();
        if playing != self.awake {
            self.awake = playing;
            window.set_keep_awake(playing);
        }
        let me = cx.entity();
        let p = theme::palette(cx);
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
        let drag_me = me.clone();
        let info = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                // Level with the note's capitals, on the device grid.
                let r = style::snap_centred(window, point(b.left() + px(8.0), b.center().y), 16.0, 16.0);
                icons::paint(window, r, icon);
                let x = r.right() + px(2.0 * m::TIGHT);
                let at = style::label_fit(
                    window,
                    cx,
                    x,
                    b.center().y,
                    Align::Left,
                    note.clone(),
                    style::PLAIN_SIZE,
                    style::ui_font(FontWeight::NORMAL),
                    style::disabled_text(&p),
                    (b.right() - x).max(px(0.0)),
                );
                olo_ui::agent::record(window, cx, olo_ui::agent::Node::label(note.clone(), at));
                // The icon is the item: drag it out, as a document's proxy
                // icon.
                let mv = drag_me.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                    if phase != gpui::DispatchPhase::Bubble || ev.pressed_button != Some(MouseButton::Left) {
                        return;
                    }
                    let Some(at) = mv.read(cx).press else { return };
                    if (ev.position - at).magnitude() < 3.0 {
                        return;
                    }
                    mv.update(cx, |q, _| q.press = None);
                    let d = mv.read(cx).drag(point(px(8.0), px(8.0)));
                    items::start(window, cx, mv.entity_id(), d);
                });
            },
        )
        .size_full();
        let grab_me = me.clone();
        let grab = div().absolute().left_0().top_0().bottom_0().w(px(20.0)).on_mouse_down(MouseButton::Left, move |ev, _, cx| grab_me.update(cx, |q, _| q.press = Some(ev.position)));
        let mut buttons = div().flex().flex_row().items_center().gap(px(m::TIGHT));
        if rotate {
            for (right, tip, glyph_right) in [(false, "Rotate Left (⌘L)", false), (true, "Rotate Right (⌘R)", true)] {
                let me = me.clone();
                buttons = buttons.child(
                    div()
                        .id(if right { "rotate-right" } else { "rotate-left" })
                        .relative()
                        .tooltip(tooltip(tip))
                        .child(Button::new("").min_width(pw).on_click(move |_, cx| me.update(cx, |q, cx| q.rotate(right, cx))))
                        .child(canvas(|_, _, _| {}, move |b, _, window, cx| rotate_glyph(window, b, glyph_right, &theme::palette(cx))).absolute().inset_0()),
                );
            }
            buttons = buttons.child(div().w(px(m::SPACING - m::TIGHT)));
        }
        if pop {
            let me = me.clone();
            buttons = buttons.child(
                div()
                    .id("pop-out")
                    .relative()
                    .tooltip(tooltip("Open in Viewer"))
                    .child(Button::new("").min_width(pw).on_click(move |_, cx| me.update(cx, |q, cx| q.act(Action::PopOut, cx))))
                    .child(canvas(|_, _, _| {}, |b, _, window, cx| pop_out_glyph(window, b, &theme::palette(cx))).absolute().inset_0()),
            );
        }
        let open_me = me.clone();
        let label_w = style::text_width(window, label.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL));
        // Its secondary button: Open With ▸ the kind's apps.
        let with_me = me.clone();
        let open_with = crate::folder::open_with_menu(std::slice::from_ref(&self.entry));
        let open_button = div().on_mouse_down(MouseButton::Right, move |ev, window, cx| {
            let Some(menu) = open_with.clone() else { return };
            let me = with_me.clone();
            olo_ui::menu::context_menu(window, cx, olo_ui::Menu::new("").submenu(menu), ev.position, move |cmd, _, cx| me.update(cx, |q, cx| q.act(Action::OpenWith(cmd.to_owned()), cx)));
        });
        let open_button = open_button.child(Button::new(label).min_width((f32::from(label_w) + 28.0).max(80.0)).on_click(move |_, cx| open_me.update(cx, |q, cx| q.act(Action::Open, cx))));
        buttons = buttons.child(div().w(px(m::SPACING - m::TIGHT))).child(open_button);
        let edge = canvas(
            |_, _, _| {},
            move |b, _, window, _| {
                style::fill_rect(window, b, style::c(p.panel_background));
                style::hline(window, b.left(), b.right(), b.top(), style::shade(&p, p.panel_background, 1.2));
                style::hline(window, b.left(), b.right(), b.top() + style::hairline(window), style::c(p.shine));
            },
        )
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
        div().size_full().flex().flex_col().track_focus(&self.focus).on_key_down(cx.listener(Self::key_down)).child(div().flex_1().min_h_0().relative().child(self.preview.clone())).child(footer)
    }
}

/// A three-quarter circle with an arrowhead, turning left or right.
fn rotate_glyph(window: &mut Window, r: Bounds<Pixels>, right: bool, p: &olo_ui::palette::Palette) {
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
fn pop_out_glyph(window: &mut Window, r: Bounds<Pixels>, p: &olo_ui::palette::Palette) {
    let ink = style::c(p.control_text);
    let o = point(style::snap_coord(window, r.center().x - px(6.0)), style::snap_coord(window, r.center().y - px(5.0)));
    let h = |window: &mut Window, x0: f32, x1: f32, y: f32| style::hline(window, o.x + px(x0), o.x + px(x1), o.y + px(y), ink);
    let v = |window: &mut Window, x: f32, y0: f32, y1: f32| style::vline(window, o.x + px(x), o.y + px(y0), o.y + px(y1), ink);
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
pub fn screen(window: &Window, cx: &App) -> Option<(u32, u32)> {
    let size = |d: &std::rc::Rc<dyn gpui::PlatformDisplay>| {
        let b = d.bounds();
        (f32::from(b.size.width) as u32, f32::from(b.size.height) as u32)
    };
    window.display(cx).map(|d| size(&d)).or_else(|| cx.displays().iter().map(size).min_by_key(|a| a.0 * a.1))
}

/// Where a Quick Look window opens: `size`, as a window of it.
pub fn bounds_size(w: u32, h: u32) -> gpui::Size<Pixels> {
    size(px(w as f32), px(h as f32))
}
