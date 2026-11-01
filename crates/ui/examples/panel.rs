//! The file panels (Open, Save, a folder), as the egui kit's demo opens
//! them, for comparing: each answer is said under the buttons.

use olo_ui::{
    WindowOptions,
    file_panel::{self, Choice, FilePanel, Filter},
    gpui::{AppContext as _, Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div, px},
    metrics as m,
    widgets::{Button, StringView},
};

struct Panels {
    said: SharedString,
}

fn home() -> std::path::PathBuf {
    std::env::var_os("HOME").map(Into::into).unwrap_or_else(|| "/".into())
}

impl Render for Panels {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let answer = move |label: &'static str| {
            let view = view.clone();
            move |a: file_panel::Answer, cx: &mut olo_ui::gpui::App| {
                view.update(cx, |p, cx| {
                    p.said = format!("{label}: {:?}", a.result).into();
                    cx.notify();
                })
            }
        };
        let (open, save, folder) = (answer("Open"), answer("Save"), answer("Folder"));
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
                    .child(Button::new("Open…").on_click(move |_, cx| {
                        let filters = vec![Filter::new("All Files").glob("*"), Filter::new("Pictures").mime("image/*"), Filter::new("Text").mime("text/*")];
                        let choices = vec![Choice { id: "ro".into(), label: "Open read-only".into(), options: vec![], value: "false".into() }];
                        let open = open.clone();
                        file_panel::show(cx, "Open", FilePanel::open(home()).filters(filters, 0).choices(choices).multiple(), move |a, cx| open(a, cx));
                    }))
                    .child(Button::new("Save…").on_click(move |_, cx| {
                        let save = save.clone();
                        file_panel::show(cx, "Save", FilePanel::save(home(), "Untitled.txt"), move |a, cx| save(a, cx));
                    }))
                    .child(Button::new("Folder…").on_click(move |_, cx| {
                        let folder = folder.clone();
                        file_panel::show(cx, "Choose a Folder", FilePanel::folder(home()), move |a, cx| folder(a, cx));
                    })),
            )
            .child(StringView::new(self.said.clone()).dim(true))
    }
}

fn main() {
    olo_ui::run("olo.panel-example", |cx| {
        olo_ui::open_window(cx, WindowOptions::new("Panels", 420.0, 120.0), |_, cx| cx.new(|_| Panels { said: "Nothing chosen yet.".into() }));
    });
}
