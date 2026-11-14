//! The column list view: processes in columns, under a menu bar, with
//! a context menu and a note of what was opened or chosen (to compare
//! with a build from before a change: `tools/dev/kit-compare`).
//! `LIST_ROWS=N` makes it N rows long (to scroll, and to measure).

use olo_kit::{
    Cx, Element, MenuBar, ParentElement as _, Pixels, Point, SharedString, Styled as _, View, WindowOptions, div,
    menu::{self, Menu},
    metrics as m, px,
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
    list: ListModel,
    last: SharedString,
    wrap: bool,
    rows: usize,
}

enum Msg {
    Opened(usize),
    Context(Point<Pixels>),
    Chose(String),
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
                .app_items(&[])
                .item("New", "new")
                .key("n")
                .item("Open…", "open")
                .key("o")
                .separator()
                .submenu(Menu::new("Recent").item("one.txt", "r1").item("two.txt", "r2"))
                .close_quit(),
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
                .item("Select All", "select-all")
                .key("a"),
            Menu::new("Options").item("Wrap Lines", "wrap").checked(self.wrap),
        ]
    }
}

impl View for Lists {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Opened(r) => self.last = row(r)[0].clone().into(),
            Msg::Chose(command) => self.last = format!("chose {command}").into(),
            Msg::Context(at) => {
                let menu = Menu::new("")
                    .item("Quit Application", "quit-app")
                    .item("Kill Process", "kill")
                    .separator()
                    .submenu(Menu::new("Priority").item("Low", "low").item("Normal", "normal").checked(true));
                let chosen = cx.msg_with(|command: &str| Msg::Chose(command.to_string()));
                menu::context_menu(cx, menu, at, chosen);
            }
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        match command {
            "quit" => olo_kit::quit(),
            "close" => cx.close_window(),
            _ => {
                if command == "wrap" {
                    self.wrap = !self.wrap;
                }
                self.last = format!("command {command}").into();
                cx.invalidate();
            }
        }
        true
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let columns = [Column::new("Name", 160.0), Column::new("CPU", 70.0).right(), Column::new("Memory", 90.0).right()];
        let list = ColumnListView::new(cx, "procs", &self.list, columns, self.rows, |_: &Self, cell, g| {
            cell.text(g, row(cell.row)[cell.column].clone());
        })
        .row_key(|_, r| r as u64)
        .type_select(|_, r| row(r)[0].clone().into())
        .on_open(cx.msg_with(|r: &usize| Msg::Opened(*r)))
        .on_context(cx.msg_with(|(_, at): &(usize, Point<Pixels>)| Msg::Context(*at)));
        let content = div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(div().flex_1().child(list)).child(StringView::new(format!("Opened: {}", self.last)).dim(true));
        div().size_full().flex().flex_col().child(MenuBar::new(cx, self.menus())).child(content)
    }
}

fn main() {
    olo_kit::run("olo.list-example", |_| {
        let rows = std::env::var("LIST_ROWS").ok().and_then(|n| n.parse().ok()).unwrap_or(ROWS.len());
        olo_kit::open_window(WindowOptions::new("Lists", 380.0, 326.0), move |_| Lists { list: ListModel::new(true), last: "nothing".into(), wrap: true, rows });
    });
}
