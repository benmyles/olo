//! The GPUI kit's column list view: processes in columns, under a menu
//! bar, with a context menu and a note of what was opened or chosen.
//! Laid out exactly as olo-kit's `list` example lays out its own: the
//! two are compared pixel for pixel (`tools/dev/kit-compare`).
//! `LIST_ROWS=N` makes it N rows long (to scroll, and to measure).

use std::rc::Rc;

use olo_ui::{
    MenuBar, WindowOptions,
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
    wrap: bool,
    rows: usize,
}

/// Row `r`'s cells.
fn row(r: usize) -> [String; 3] {
    let (name, cpu, memory) = ROWS[r % ROWS.len()];
    [if r < ROWS.len() { name.to_string() } else { format!("{name} {}", r + 1) }, cpu.to_string(), memory.to_string()]
}

impl Lists {
    fn menus(&self) -> Vec<Menu> {
        vec![
            Menu::new("File")
                .item("New", "new")
                .key("n")
                .item("Open…", "open")
                .key("o")
                .separator()
                .submenu(Menu::new("Recent").item("one.txt", "r1").item("two.txt", "r2"))
                .separator()
                .item("Close", "close")
                .key("w")
                .item("Quit", "quit")
                .key("q"),
            Menu::new("Edit")
                .item("Undo", "undo")
                .key("z")
                .item("Redo", "redo")
                .shift_key("z")
                .separator()
                .item("Cut", "cut")
                .key("x")
                .item("Copy", "copy")
                .key("c")
                .item("Paste", "paste")
                .key("v")
                .item("Select All", "all")
                .key("a"),
            Menu::new("Options").item("Wrap Lines", "wrap").checked(self.wrap),
        ]
    }
}

impl Render for Lists {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = [Column::new("Name", 160.0), Column::new("CPU", 70.0).right(), Column::new("Memory", 90.0).right()];
        let (opened, chosen) = (cx.entity(), cx.entity());
        let list = ColumnListView::new("procs", &self.list, columns, self.rows, |cell, window, cx| {
            cell.text(window, cx, row(cell.row)[cell.column].clone());
        })
        .row_key(|r, _| r as u64)
        .type_select(|r| row(r)[0].clone().into())
        .on_open(move |r, _, cx| {
            opened.update(cx, |l, cx| {
                l.last = row(r)[0].clone().into();
                cx.notify();
            })
        })
        .on_context(move |_, at, window, cx| {
            let chosen = chosen.clone();
            let menu =
                Menu::new("").item("Quit Application", "quit-app").item("Kill Process", "kill").separator().submenu(Menu::new("Priority").item("Low", "low").item("Normal", "normal").checked(true));
            menu::context_menu(window, cx, menu, at, move |command, _, cx| {
                chosen.update(cx, |l, cx| {
                    l.last = format!("chose {command}").into();
                    cx.notify();
                })
            });
        });
        let rows: Rc<str> = format!("Opened: {}", self.last).into();
        let me = cx.entity();
        let bar = MenuBar::new(self.menus(), move |command, window, cx| match command {
            "quit" => cx.quit(),
            "close" => window.remove_window(),
            _ => me.update(cx, |l, cx| {
                if command == "wrap" {
                    l.wrap = !l.wrap;
                }
                l.last = format!("command {command}").into();
                cx.notify();
            }),
        });
        let content = div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(div().flex_1().child(list)).child(StringView::new(rows.to_string()).dim(true));
        div().size_full().flex().flex_col().child(bar).child(content)
    }
}

fn main() {
    olo_ui::run("olo.list-example", |cx| {
        olo_ui::open_window(cx, WindowOptions::new("Lists", 380.0, 326.0), |_, cx| {
            let list = ListModel::new(cx, true);
            let rows = std::env::var("LIST_ROWS").ok().and_then(|n| n.parse().ok()).unwrap_or(ROWS.len());
            cx.new(|_| Lists { list, last: "nothing".into(), wrap: true, rows })
        });
    });
}
