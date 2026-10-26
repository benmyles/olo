use tabula_kit::{
    App, Cx, Menu, WindowId, WindowOptions, egui,
    widgets::{
        BBox, Button, CheckBox, Column, ColumnListView, ListState, MenuField, RadioButton, Slider, StatusBar,
        TabView, TextControl, TextView,
    },
};

struct Demo {
    name: String,
    wrap: bool,
    sound: bool,
    volume: f32,
    choice: usize,
    radio: usize,
    tab: usize,
    notes: String,
    list: ListState,
    last: String,
}

const FONTS: &[&str] = &["Nimbus Sans", "DejaVu Sans", "Noto Sans", "Liberation Sans"];
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

impl App for Demo {
    fn ui(&mut self, cx: &mut Cx, _w: WindowId, ui: &mut egui::Ui) {
        tabula_kit::widgets::window_content(ui, |ui| {
            TabView::new(&mut self.tab, &["Controls", "Lists", "Text"]).show(ui, |ui, tab| match tab {
                0 => {
                    ui.add(TextControl::new(&mut self.name).label("Name:").divider(60.0));
                    ui.add(MenuField::new("font", &mut self.choice, FONTS).label("Font:").divider(60.0));
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        BBox::new("Options").show(ui, |ui| {
                            ui.set_width(180.0);
                            ui.add(CheckBox::new(&mut self.wrap, "Wrap lines"));
                            ui.add(CheckBox::new(&mut self.sound, "Play sounds"));
                            ui.add_enabled(false, CheckBox::new(&mut true.clone(), "Disabled"));
                        });
                        BBox::new("Mode").show(ui, |ui| {
                            for (i, l) in ["Light", "Dark", "Automatic"].iter().enumerate() {
                                if ui.add(RadioButton::new(self.radio == i, l)).clicked() {
                                    self.radio = i;
                                }
                            }
                        });
                    });
                    let vol = format!("{:.0} %", self.volume);
                    ui.add(
                        Slider::new(&mut self.volume, 0.0..=100.0)
                            .label("Volume")
                            .value_text(vol)
                            .limits("Quiet", "Loud"),
                    );
                    ui.add(StatusBar::new(self.volume / 100.0).label("Copying files…").trailing("3 of 7"));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.add(Button::new("Copy Name")).clicked() {
                            cx.copy(self.name.clone());
                        }
                        ui.add_enabled(false, Button::new("Disabled"));
                        ui.add(Button::new("OK").default(true));
                    });
                }
                1 => {
                    let columns = [Column::new("Name", 160.0), Column::new("CPU", 70.0).right(), Column::new("Memory", 90.0).right()];
                    let r = ColumnListView::new("procs", &columns, ROWS.len(), &mut self.list).show(ui, |cell| {
                        let row = ROWS[cell.row];
                        cell.text([row.0, row.1, row.2][cell.column]);
                    });
                    if let Some(a) = r.activated {
                        self.last = ROWS[a].0.to_owned();
                    }
                    let menu_id = egui::Id::unique("proc-menu");
                    if let Some((_, pos)) = r.context {
                        tabula_kit::widgets::popup_menu_at(
                            ui.ctx(),
                            menu_id,
                            pos,
                            Menu::new("")
                                .item("Quit Application", "quit-app", None)
                                .item("Kill Process", "kill", None)
                                .separator()
                                .submenu(Menu::new("Priority").item("Low", "low", None).item("Normal", "normal", None).checked(true)),
                        );
                    }
                    if let Some(cmd) = tabula_kit::widgets::popup_result(ui.ctx(), menu_id) {
                        eprintln!("context: {cmd}");
                    }
                }
                _ => {
                    TextView::new("notes", &mut self.notes).show(ui);
                }
            });
        });
    }

    fn menu_bar(&mut self, _w: WindowId) -> Option<Vec<Menu>> {
        Some(vec![
            Menu::new("File")
                .item("New", "new", Some(egui::Key::N))
                .item("Open…", "open", Some(egui::Key::O))
                .separator()
                .submenu(Menu::new("Recent").item("one.txt", "r1", None).item("two.txt", "r2", None))
                .separator()
                .item("Close", "close", Some(egui::Key::W))
                .item("Quit", "quit", Some(egui::Key::Q)),
            Menu::new("Edit")
                .item("Undo", "undo", Some(egui::Key::Z))
                .item_shift("Redo", "redo", egui::Key::Z)
                .separator()
                .item("Cut", "cut", Some(egui::Key::X))
                .item("Copy", "copy", Some(egui::Key::C))
                .item("Paste", "paste", Some(egui::Key::V))
                .item("Select All", "all", Some(egui::Key::A)),
            Menu::new("Options").item("Wrap Lines", "wrap", None).checked(self.wrap),
        ])
    }

    fn command(&mut self, cx: &mut Cx, w: WindowId, command: &str) {
        match command {
            "quit" => cx.quit(),
            "close" => cx.close_window(w),
            "wrap" => self.wrap = !self.wrap,
            "new" => {
                cx.open_window(WindowOptions::new("Another", 360, 240));
            }
            _ => {}
        }
    }
}

fn main() {
    tabula_kit::run("tabula.kit-demo", |cx| {
        cx.open_window(WindowOptions::new("Kit Demo", 460, 420));
        Demo {
            name: "Hello, BeOS".into(),
            wrap: true,
            sound: false,
            volume: 42.0,
            choice: 0,
            radio: 0,
            tab: 0,
            notes: "The quick brown fox jumps over the lazy dog.\n\nBeOS was a joy to use.\n".repeat(20),
            list: ListState { multiple: true, ..Default::default() },
            last: String::new(),
        }
    });
}
