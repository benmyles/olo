//! The file panels (Open, Save, a folder), each a button away: each
//! answer is said under the buttons (to compare with a build from
//! before a change: `tools/dev/kit-compare`).

use olo_kit::{
    ClickEvent, Cx, Element, ParentElement as _, SharedString, Styled as _, View, WindowOptions, div,
    file_panel::{self, Choice, FilePanel, Filter},
    metrics as m, px,
    widgets::{Button, StringView},
};

struct Panels {
    said: SharedString,
}

#[derive(Clone)]
enum Msg {
    Open,
    Save,
    Folder,
    Said(String),
}

fn home() -> std::path::PathBuf {
    std::env::var_os("HOME").map(Into::into).unwrap_or_else(|| "/".into())
}

impl View for Panels {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        // (The panel answers on its own looper: it tells this view.)
        let to = cx.messenger();
        let answer = move |label: &'static str| {
            move |a: file_panel::Answer| {
                to.send(Msg::Said(format!("{label}: {:?}", a.result)));
            }
        };
        match message {
            Msg::Open => {
                let filters = vec![Filter::new("All Files").glob("*"), Filter::new("Pictures").mime("image/*"), Filter::new("Text").mime("text/*")];
                let choices = vec![Choice { id: "ro".into(), label: "Open read-only".into(), options: vec![], value: "false".into() }];
                file_panel::show("Open", FilePanel::open(home()).filters(filters, 0).choices(choices).multiple(), answer("Open"));
            }
            Msg::Save => {
                file_panel::show("Save", FilePanel::save(home(), "Untitled.txt"), answer("Save"));
            }
            Msg::Folder => {
                file_panel::show("Choose a Folder", FilePanel::folder(home()), answer("Folder"));
            }
            Msg::Said(said) => {
                self.said = said.into();
                cx.invalidate();
            }
        }
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .p(px(m::WINDOW_INSET))
            .child(
                div()
                    .flex()
                    .gap(px(m::SPACING))
                    .child(Button::new("Open…").on_click(cx.msg::<ClickEvent>(Msg::Open)))
                    .child(Button::new("Save…").on_click(cx.msg::<ClickEvent>(Msg::Save)))
                    .child(Button::new("Folder…").on_click(cx.msg::<ClickEvent>(Msg::Folder))),
            )
            .child(StringView::new(self.said.clone()).dim(true))
    }
}

fn main() {
    olo_kit::run("olo.panel-example", |_| {
        olo_kit::open_window(WindowOptions::new("Panels", 420.0, 120.0), |_| Panels { said: "Nothing chosen yet.".into() });
    });
}
