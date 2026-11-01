//! The GPUI kit's widgets in one window, laid out as the egui kit's demo
//! (crates/kit/examples/demo.rs, its Controls tab), for comparing the
//! two closely (`tsession` at every scale and theme).

use olo_ui::{
    WindowOptions,
    gpui::{AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString, Styled as _, Window, div, px},
    metrics as m,
    widgets::{BBox, Button, CheckBox, Column, ColumnListView, Graph, ListModel, MenuField, RadioButton, ScrollView, Series, Slider, StatusBar, StringView, TextControl, TextField, open_alert, text_field},
};
use std::{collections::VecDeque, rc::Rc};

const FONTS: &[&str] = &["Nimbus Sans", "DejaVu Sans", "Noto Sans", "Liberation Sans"];

struct Gallery {
    name: Entity<TextField>,
    wrap: bool,
    sound: bool,
    radio: usize,
    choice: usize,
    rows: ScrollHandle,
    answer: String,
    volume: f32,
    files: Entity<ListModel>,
    /// Name, size in bytes, kind: sorted as the list's header says.
    entries: Rc<Vec<(SharedString, u64, SharedString)>>,
}

/// Some files for the list.
fn entries() -> Vec<(SharedString, u64, SharedString)> {
    let kinds = ["Text", "Folder", "Picture", "Application", "Archive"];
    let names = ["apps", "Cargo.toml", "crates", "docs", "gpui", "README.md", "screenshot.png", "olo-terminal", "notes.txt", "backup.tar.zst", "tools", "vendor", "wallpaper.jpg", "a file whose name is too long to fit its column", "LICENSE", "target"];
    names.iter().enumerate().map(|(i, n)| ((*n).into(), (i as u64 * 7919 + 13).pow(2) % 9_000_000, kinds[i % kinds.len()].into())).collect()
}

fn size_text(bytes: u64) -> String {
    if bytes >= 1_000_000 { format!("{:.1} MB", bytes as f64 / 1e6) } else { format!("{} KB", bytes.div_ceil(1000)) }
}

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let set = move |f: fn(&mut Gallery, bool)| {
            let view = view.clone();
            move |on: bool, _: &mut Window, cx: &mut olo_ui::gpui::App| {
                view.update(cx, |g, cx| {
                    f(g, on);
                    cx.notify();
                })
            }
        };
        let radio = |i: usize| {
            let set = set(|_, _| {});
            let view = cx.entity();
            move |window: &mut Window, cx: &mut olo_ui::gpui::App| {
                view.update(cx, |g, _| g.radio = i);
                set(true, window, cx);
            }
        };
        let choose = {
            let view = cx.entity();
            move |i: usize, _: &mut Window, cx: &mut olo_ui::gpui::App| {
                view.update(cx, |g, cx| {
                    g.choice = i;
                    cx.notify();
                })
            }
        };
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(m::SPACING))
            .p(px(m::WINDOW_INSET))
            .child(TextControl::new(self.name.clone()).label("Name:").divider(60.0))
            .child(MenuField::new("font", FONTS.iter().map(|&f| f.into()).collect(), self.choice).label("Font:").divider(60.0).on_change(choose))
            .child(
                div()
                    .mt(px(4.0))
                    .flex()
                    .gap(px(m::SPACING))
                    .child(
                        BBox::new("Options")
                            .content_width(180.0)
                            .child(CheckBox::new("Wrap lines", self.wrap).on_toggle(set(|g, on| g.wrap = on)))
                            .child(CheckBox::new("Play sounds", self.sound).on_toggle(set(|g, on| g.sound = on)))
                            .child(CheckBox::new("Disabled", true).enabled(false)),
                    )
                    .child(
                        BBox::new("Mode")
                            .child(RadioButton::new("Light", self.radio == 0).on_select(radio(0)))
                            .child(RadioButton::new("Dark", self.radio == 1).on_select(radio(1)))
                            .child(RadioButton::new("Automatic", self.radio == 2).on_select(radio(2))),
                    ),
            )
            .child(
                div()
                    .mt(px(6.0))
                    .flex()
                    .gap(px(m::SPACING))
                    .child(Button::new("Alert…").on_click({
                        let view = cx.entity();
                        move |_, cx| {
                            let view = view.clone();
                            open_alert(cx, "Gallery", "warning", "Something needs an answer. This alert's message is long enough to wrap onto a second line.", &["Cancel", "Other", "OK"], move |choice, cx| {
                                view.update(cx, |g, cx| {
                                    g.answer = format!("{choice:?}");
                                    cx.notify();
                                })
                            });
                        }
                    }))
                    .child(Button::new("Copy Name"))
                    .child(Button::new("Disabled").enabled(false))
                    .child(Button::new("OK").default(true)),
            )
            .child(StringView::new(format!("Alert answer: {}", self.answer)).dim(true))
            .child(div().w(px(300.0)).child({
                let view = cx.entity();
                Slider::new("volume", self.volume, 0.0..=100.0).label("Volume").value_text(format!("{:.0} %", self.volume)).limits("Quiet", "Loud").step(1.0).on_change(move |v, _, cx| {
                    view.update(cx, |g, cx| {
                        g.volume = v;
                        cx.notify();
                    })
                })
            }))
            .child(div().w(px(300.0)).child(StatusBar::new(0.42).label("Copying files…").trailing("3 of 7")))
            .child(div().h(px(100.0)).w(px(220.0)).child(ScrollView::new("rows", &self.rows).child(div().px(px(6.0)).children((1..=40).map(|i| StringView::new(format!("Row {i}")))))))
            .child(div().h(px(140.0)).w(px(300.0)).child(self.list(cx)))
            .child(div().h(px(80.0)).w(px(300.0)).child(graph(cx)))
    }
}

/// A minute of two made-up series, one filled.
fn graph(cx: &mut Context<Gallery>) -> Graph {
    let p = olo_ui::theme::palette(cx);
    let wave = |f: f64, k: f64| Rc::new((0..=120).map(|i| (i as f64 * 0.5, k * (1.2 + (i as f64 * f).sin() + 0.3 * (i as f64 * f * 3.1).cos()))).collect::<VecDeque<_>>());
    let series = [
        Series::new("In", olo_ui::style::c(p.hue(olo_ui::palette::Hue::Blue)), wave(0.11, 40.0)).fill(true),
        Series::new("Out", olo_ui::style::c(p.hue(olo_ui::palette::Hue::Red)), wave(0.07, 25.0)),
    ];
    Graph::new("gallery-graph", series, 60.0).span(60.0).grid(15.0)
}

impl Gallery {
    fn list(&self, cx: &mut Context<Self>) -> ColumnListView {
        let columns = [Column::new("Name", 150.0), Column::new("Size", 64.0).right(), Column::new("Kind", 80.0)];
        let (cells, names) = (self.entries.clone(), self.entries.clone());
        let view = cx.entity();
        ColumnListView::new("files", &self.files, columns, self.entries.len(), move |cell, window, cx| {
            let (name, size, kind) = &cells[cell.row];
            match cell.column {
                0 => cell.text(window, cx, name.clone()),
                1 => cell.text(window, cx, size_text(*size)),
                _ => cell.text(window, cx, kind.clone()),
            }
        })
        .type_select(move |r| names[r].0.clone())
        .empty_text("No files.")
        .on_sort(move |_, cx| {
            view.update(cx, |g, cx| {
                g.sort(cx);
                cx.notify();
            })
        })
    }

    /// The entries in the header's order, the selection kept on them.
    fn sort(&mut self, cx: &mut Context<Self>) {
        let state = self.files.read(cx).state.clone();
        let Some((col, ascending)) = state.sort else { return };
        let selected: Vec<SharedString> = state.selected.iter().map(|&r| self.entries[r].0.clone()).collect();
        let mut entries = (*self.entries).clone();
        entries.sort_by(|a, b| {
            let o = match col {
                0 => a.0.to_lowercase().cmp(&b.0.to_lowercase()),
                1 => a.1.cmp(&b.1),
                _ => a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)),
            };
            if ascending { o } else { o.reverse() }
        });
        self.files.update(cx, |m, _| m.state.selected = entries.iter().enumerate().filter(|(_, e)| selected.contains(&e.0)).map(|(i, _)| i).collect());
        self.entries = Rc::new(entries);
    }
}

fn main() {
    olo_ui::run("olo.gallery", |cx| {
        olo_ui::open_window(cx, WindowOptions::new("Gallery", 480.0, 770.0), |_, cx| {
            let name = text_field(cx, |f| f.name("Name"));
            name.update(cx, |f, cx| f.set_text("Hello, BeOS", cx));
            let files = ListModel::new(cx, true);
            files.update(cx, |m, _| m.state.sort = Some((0, true)));
            let mut g = Gallery { name, wrap: true, sound: false, radio: 0, choice: 0, rows: ScrollHandle::new(), answer: "none".into(), volume: 42.0, files, entries: Rc::new(entries()) };
            cx.new(|cx| {
                g.sort(cx);
                g
            })
        });
    });
}
