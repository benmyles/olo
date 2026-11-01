//! The column list view, as the egui kit's demo shows one (its Lists
//! tab: the same processes and columns), for comparing the two closely,
//! with a context menu and a note of what was opened.

use std::rc::Rc;

use olo_ui::{
    WindowOptions,
    gpui::{AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div, px},
    menu::{self, Menu},
    metrics as m,
    widgets::{Column, ColumnListView, ListModel, StringView},
};

const ROWS: &[(&str, &str, &str)] = &[
    ("app_server", "1.2 %", "48.3 MB"),
    ("Tracker", "0.4 %", "31.0 MB"),
    ("Deskbar", "0.1 %", "12.7 MB"),
    ("Terminal", "2.8 %", "64.1 MB"),
    ("WebPositive", "11.5 %", "412.9 MB"),
    ("StyledEdit", "0.0 %", "9.4 MB"),
    ("Pulse", "0.3 %", "6.2 MB"),
    ("media_server", "0.9 %", "22.0 MB"),
    ("net_server", "0.2 %", "8.8 MB"),
    ("registrar", "0.0 %", "4.1 MB"),
    ("input_server", "0.6 %", "11.3 MB"),
    ("syslog_daemon", "0.0 %", "2.0 MB"),
];

struct Lists {
    list: Entity<ListModel>,
    last: SharedString,
}

impl Render for Lists {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = [Column::new("Name", 160.0), Column::new("CPU", 70.0).right(), Column::new("Memory", 90.0).right()];
        let (opened, chosen) = (cx.entity(), cx.entity());
        let list = ColumnListView::new("procs", &self.list, columns, ROWS.len(), |cell, window, cx| {
            let row = ROWS[cell.row];
            cell.text(window, cx, [row.0, row.1, row.2][cell.column]);
        })
        .type_select(|r| ROWS[r].0.into())
        .on_open(move |r, _, cx| {
            opened.update(cx, |l, cx| {
                l.last = ROWS[r].0.into();
                cx.notify();
            })
        })
        .on_context(move |_, at, window, cx| {
            let chosen = chosen.clone();
            let menu = Menu::new("").item("Quit Application", "quit-app").item("Kill Process", "kill").separator().submenu(Menu::new("Priority").item("Low", "low").item("Normal", "normal").checked(true));
            menu::context_menu(window, cx, menu, at, move |command, _, cx| {
                chosen.update(cx, |l, cx| {
                    l.last = format!("chose {command}").into();
                    cx.notify();
                })
            });
        });
        let rows: Rc<str> = format!("Opened: {}", self.last).into();
        div().size_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(div().flex_1().child(list)).child(StringView::new(rows.to_string()).dim(true))
    }
}

fn main() {
    olo_ui::run("olo.list-example", |cx| {
        olo_ui::open_window(cx, WindowOptions::new("Lists", 380.0, 300.0), |_, cx| {
            let list = ListModel::new(cx, true);
            cx.new(|_| Lists { list, last: "nothing".into() })
        });
    });
}
