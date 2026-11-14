//! The kit's widgets in one window, for looking at them closely
//! (`tsession` at every scale and theme), and the speed benchmark's
//! kit app (`tools/dev/bench`).

use std::{collections::VecDeque, rc::Rc};

use olo_kit::{
    Child, Cx, Element, ParentElement as _, ScrollHandle, SharedString, Styled as _, View, WindowOptions, div, metrics as m,
    palette::Hue,
    px, style,
    widgets::{
        BBox, Button, CheckBox, Column, ColumnListView, Graph, ListModel, MenuField, RadioButton, ScrollView, Series, Slider, StatusBar, StringView, TextControl, TextField, open_alert, text_field,
    },
};

const FONTS: &[&str] = &["Nimbus Sans", "DejaVu Sans", "Noto Sans", "Liberation Sans"];

struct Gallery {
    name: Child<TextField>,
    wrap: bool,
    sound: bool,
    radio: usize,
    choice: usize,
    rows: ScrollHandle,
    answer: String,
    volume: f32,
    files: ListModel,
    /// Name, size in bytes, kind: sorted as the list's header says.
    entries: Vec<(SharedString, u64, SharedString)>,
}

#[derive(Clone)]
enum Msg {
    Wrap(bool),
    Sound(bool),
    Radio(usize),
    Font(usize),
    Volume(f32),
    Ask,
    Answered(Option<usize>),
    Sorted,
}

/// Some files for the list.
fn entries() -> Vec<(SharedString, u64, SharedString)> {
    let kinds = ["Text", "Folder", "Picture", "Application", "Archive"];
    let names = [
        "apps",
        "Cargo.toml",
        "crates",
        "docs",
        "smithay",
        "README.md",
        "screenshot.png",
        "olo-terminal",
        "notes.txt",
        "backup.tar.zst",
        "tools",
        "vendor",
        "wallpaper.jpg",
        "a file whose name is too long to fit its column",
        "LICENSE",
        "target",
    ];
    names.iter().enumerate().map(|(i, n)| ((*n).into(), (i as u64 * 7919 + 13).pow(2) % 9_000_000, kinds[i % kinds.len()].into())).collect()
}

fn size_text(bytes: u64) -> String {
    if bytes >= 1_000_000 { format!("{:.1} MB", bytes as f64 / 1e6) } else { format!("{} KB", bytes.div_ceil(1000)) }
}

impl View for Gallery {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Wrap(on) => self.wrap = on,
            Msg::Sound(on) => self.sound = on,
            Msg::Radio(i) => self.radio = i,
            Msg::Font(i) => self.choice = i,
            Msg::Volume(v) => self.volume = v,
            Msg::Ask => {
                let answered = cx.msg_with(|choice: &Option<usize>| Msg::Answered(*choice));
                open_alert(cx, "Gallery", "warning", "Something needs an answer. This alert's message is long enough to wrap onto a second line.", &["Cancel", "Other", "OK"], answered);
                return;
            }
            Msg::Answered(choice) => self.answer = format!("{choice:?}"),
            Msg::Sorted => self.sort(),
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let list = self.list(cx);
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(m::SPACING))
            .p(px(m::WINDOW_INSET))
            .child(TextControl::new(cx, &self.name).label("Name:").divider(60.0))
            .child(MenuField::new("font", FONTS.iter().map(|&f| f.into()).collect(), self.choice).label("Font:").divider(60.0).on_change(cx.msg_with(|i: &usize| Msg::Font(*i))))
            .child(
                div()
                    .mt(px(4.0))
                    .flex()
                    .gap(px(m::SPACING))
                    .child(
                        BBox::new("Options")
                            .content_width(180.0)
                            .child(CheckBox::new("Wrap lines", self.wrap).on_toggle(cx.msg_with(|on: &bool| Msg::Wrap(*on))))
                            .child(CheckBox::new("Play sounds", self.sound).on_toggle(cx.msg_with(|on: &bool| Msg::Sound(*on))))
                            .child(CheckBox::new("Disabled", true).enabled(false)),
                    )
                    .child(
                        BBox::new("Mode")
                            .child(RadioButton::new("Light", self.radio == 0).on_select(cx.msg(Msg::Radio(0))))
                            .child(RadioButton::new("Dark", self.radio == 1).on_select(cx.msg(Msg::Radio(1))))
                            .child(RadioButton::new("Automatic", self.radio == 2).on_select(cx.msg(Msg::Radio(2)))),
                    ),
            )
            .child(
                div()
                    .mt(px(6.0))
                    .flex()
                    .gap(px(m::SPACING))
                    .child(Button::new("Alert…").on_click(cx.msg(Msg::Ask)))
                    .child(Button::new("Copy Name"))
                    .child(Button::new("Disabled").enabled(false))
                    .child(Button::new("OK").default(true)),
            )
            .child(StringView::new(format!("Alert answer: {}", self.answer)).dim(true))
            .child(
                div().w(px(300.0)).child(
                    Slider::new("volume", self.volume, 0.0..=100.0)
                        .label("Volume")
                        .value_text(format!("{:.0} %", self.volume))
                        .limits("Quiet", "Loud")
                        .step(1.0)
                        .on_change(cx.msg_with(|v: &f32| Msg::Volume(*v))),
                ),
            )
            .child(div().w(px(300.0)).child(StatusBar::new(0.42).label("Copying files…").trailing("3 of 7")))
            .child(div().h(px(100.0)).w(px(220.0)).child(ScrollView::new(&self.rows).child(div().px(px(6.0)).children((1..=40).map(|i| StringView::new(format!("Row {i}")))))))
            .child(div().h(px(140.0)).w(px(300.0)).child(list))
            .child(div().h(px(80.0)).w(px(300.0)).child(graph()))
    }
}

/// A minute of two made-up series, one filled.
fn graph() -> Graph {
    let p = olo_kit::theme::palette();
    let wave = |f: f64, k: f64| Rc::new((0..=120).map(|i| (i as f64 * 0.5, k * (1.2 + (i as f64 * f).sin() + 0.3 * (i as f64 * f * 3.1).cos()))).collect::<VecDeque<_>>());
    let series = [Series::new("In", style::c(p.hue(Hue::Blue)), wave(0.11, 40.0)).fill(true), Series::new("Out", style::c(p.hue(Hue::Red)), wave(0.07, 25.0))];
    Graph::new("gallery-graph", series, 60.0).span(60.0).grid(15.0)
}

impl Gallery {
    fn list(&self, cx: &mut Cx<Self>) -> ColumnListView<Self> {
        let columns = [Column::new("Name", 150.0), Column::new("Size", 64.0).right(), Column::new("Kind", 80.0)];
        ColumnListView::new(cx, "files", &self.files, columns, self.entries.len(), |v: &Self, cell, g| {
            let (name, size, kind) = &v.entries[cell.row];
            match cell.column {
                0 => cell.text(g, name.clone()),
                1 => cell.text(g, size_text(*size)),
                _ => cell.text(g, kind.clone()),
            }
        })
        .type_select(|v, r| v.entries[r].0.clone())
        .empty_text("No files.")
        .on_sort(cx.msg(Msg::Sorted))
    }

    /// The entries in the header's order, the selection kept on them.
    fn sort(&mut self) {
        let state = self.files.state().clone();
        let Some((col, ascending)) = state.sort else { return };
        let selected: Vec<SharedString> = state.selected.iter().map(|&r| self.entries[r].0.clone()).collect();
        self.entries.sort_by(|a, b| {
            let o = match col {
                0 => a.0.to_lowercase().cmp(&b.0.to_lowercase()),
                1 => a.1.cmp(&b.1),
                _ => a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)),
            };
            if ascending { o } else { o.reverse() }
        });
        self.files.state_mut().selected = self.entries.iter().enumerate().filter(|(_, e)| selected.contains(&e.0)).map(|(i, _)| i).collect();
    }
}

fn main() {
    olo_kit::run("olo.gallery", |_| {
        olo_kit::open_window(WindowOptions::new("Gallery", 480.0, 770.0), |cx| {
            let files = ListModel::new(true);
            files.state_mut().sort = Some((0, true));
            let mut gallery = Gallery {
                name: text_field(cx, |f| f.name("Name").with_text("Hello, BeOS")),
                wrap: true,
                sound: false,
                radio: 0,
                choice: 0,
                rows: ScrollHandle::new(),
                answer: "none".into(),
                volume: 42.0,
                files,
                entries: entries(),
            };
            gallery.sort();
            gallery
        });
    });
}
