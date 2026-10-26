use tabula_kit::{App, Cx, Menu, WindowId, WindowOptions, egui};

struct Demo {
    text: String,
    check: bool,
    value: f32,
}

impl App for Demo {
    fn ui(&mut self, cx: &mut Cx, _w: WindowId, ui: &mut egui::Ui) {
        egui::Frame::new().inner_margin(12.0).show(ui, |ui| {
            ui.heading("Interface Kit");
            ui.label("The quick brown fox jumps over the lazy dog.");
            ui.text_edit_singleline(&mut self.text);
            ui.checkbox(&mut self.check, "Enable thing");
            ui.add(egui::Slider::new(&mut self.value, 0.0..=100.0));
            if ui.button("Copy text").clicked() {
                cx.copy(self.text.clone());
            }
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
                .separator()
                .item("Cut", "cut", Some(egui::Key::X))
                .item("Copy", "copy", Some(egui::Key::C))
                .item("Paste", "paste", Some(egui::Key::V))
                .item("Select All", "all", Some(egui::Key::A)),
            Menu::new("Options").item("Wrap Lines", "wrap", None).checked(self.check),
        ])
    }

    fn command(&mut self, cx: &mut Cx, w: WindowId, command: &str) {
        eprintln!("command: {command}");
        match command {
            "quit" => cx.quit(),
            "close" => cx.close_window(w),
            "new" => {
                cx.open_window(WindowOptions::new("Another", 360, 240));
            }
            _ => {}
        }
    }
}

fn main() {
    tabula_kit::run("tabula.kit-demo", |cx| {
        cx.open_window(WindowOptions::new("Kit Demo", 420, 300));
        Demo { text: "Hello, BeOS".into(), check: true, value: 42.0 }
    });
}
