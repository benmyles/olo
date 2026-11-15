//! File ▸ Apps Allowed…: the programs allowed an item for good (Approve
//! Always, when a program asked for an item by its reference: Weather's
//! key), each taken back here (Ben, 2026-11-15: "once i allow the weather
//! app to use the passwords item it should keep that approval forever,
//! not until the next restart … make it so the dialog has both an
//! 'Approve Always' and 'Approve Once' with 'Approve Always' the primary
//! choice"). Kept in the vault (`vault::Allowed`), so across restarts and
//! upgrades; taken back, the program is asked again the next time.

use chrono::{DateTime, Local, Utc};
use olo_kit::{
    ClickEvent, Cx, Element, FocusHandle, FontWeight, InteractiveElement as _, KeyDownEvent, ParentElement as _, ScrollHandle, SharedString, Styled as _, View, agent, canvas, div, metrics as m, px,
    style, theme,
    widgets::{Button, ScrollView, button_row},
};

use crate::{app, store::Job};

/// A row's height: the program's name, and the item under it.
const ROW: f32 = 46.0;

pub struct AllowedView {
    list: ScrollHandle,
    focus: FocusHandle,
}

impl AllowedView {
    pub fn new(cx: &mut Cx<Self>) -> AllowedView {
        let focus = FocusHandle::new();
        focus.focus(cx);
        AllowedView { list: ScrollHandle::new(), focus }
    }
}

/// When it was allowed, as the person reads dates.
fn since(at: &DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("since %-d %b %Y").to_string()
}

impl View for AllowedView {
    type Message = ();

    fn message(&mut self, _: (), cx: &mut Cx<Self>) {
        cx.invalidate();
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        app().allowed_window = None;
    }

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        let a = app();
        let p = theme::palette();
        let message = if a.allowed.is_empty() {
            "No app may have an item without asking. When one asks for an item, Approve Always allows it for good, and it’s listed here to take back.".to_owned()
        } else {
            "These apps may have these items whenever they ask. Take one back and the app is asked again the next time.".to_owned()
        };
        let mut rows = div().w_full().flex().flex_col();
        for allowed in &a.allowed {
            let name = crate::program_name(&allowed.program).to_owned();
            let item = format!("{}, {}", a.reference_named(&allowed.reference), since(&allowed.since));
            let (program, reference) = (allowed.program.clone(), allowed.reference.clone());
            rows = rows.child(
                div()
                    .w_full()
                    .h(px(ROW))
                    .flex_shrink_0()
                    .px(px(m::SPACING))
                    .flex()
                    .items_center()
                    .gap(px(m::SPACING))
                    .child({
                        let (name, item): (SharedString, SharedString) = (name.into(), item.into());
                        let key = style::key((&name, &item));
                        canvas(move |g, b| {
                            let p = theme::palette();
                            let width = b.size.width;
                            let font = style::ui_font(FontWeight::NORMAL);
                            let t = style::label_fit(g, b.left(), b.center().y - px(8.0), style::Align::Left, name.clone(), style::PLAIN_SIZE, font.clone(), style::c(p.list_item_text), width);
                            let d = style::label_fit(g, b.left(), b.center().y + px(9.0), style::Align::Left, item.clone(), style::SMALL_SIZE, font, style::disabled_text(&p), width);
                            agent::record(g, agent::Node { role: "ListItem", name: Some(name.clone()), value: Some(item.clone()), bounds: t.union(&d), enabled: true, ..Default::default() });
                        })
                        .paint_key(key)
                        .flex_1()
                        .h_full()
                    })
                    .child(Button::new("Take Back").on_click(move |_: &ClickEvent, _| app().take_back(&program, &reference))),
            );
        }
        drop(a);
        let list = ScrollView::new(&self.list).background(p.list_background).child(rows);
        let buttons = button_row().child(Button::new("Done").default(true).on_click(|_: &ClickEvent, cx| cx.close_window()));
        crate::view::sheet(message, list, buttons).track_focus(&self.focus).on_key_down(|e: &KeyDownEvent, cx| {
            if e.keystroke.key == "escape" {
                cx.stop_propagation();
                cx.close_window();
            }
        })
    }
}

impl crate::Passwords {
    /// File ▸ Apps Allowed…: opened, or brought forward.
    pub fn show_allowed(&mut self) {
        if let Some(w) = self.allowed_window.as_ref().filter(|w| w.is_open()) {
            olo_kit::raise(Some(w));
            return;
        }
        let options = olo_kit::WindowOptions::new("Apps Allowed", 520.0, 300.0).min_size(400.0, 220.0).dialog().child();
        self.allowed_window = Some(olo_kit::open_window(options, AllowedView::new));
    }

    /// What `program` was allowed of `reference`, taken back.
    pub fn take_back(&mut self, program: &str, reference: &str) {
        self.allowed.retain(|a| (a.program.as_str(), a.reference.as_str()) != (program, reference));
        self.store.send(Job::TakeBack { program: program.to_owned(), reference: reference.to_owned() });
        self.changed();
    }
}
