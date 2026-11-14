//! A window's tool bars (along the top, wrapped at its separators, and
//! down a side) and text views (coloured, and empty) (to compare with a
//! build from before a change: `tools/dev/kit-compare`).

use olo_kit::{
    Child, Cx, Element, Menu, ParentElement as _, Styled as _, TextRun, View, WindowOptions, div, hsla, metrics as m, px, style,
    widgets::{StringView, TextView, ToolBar, text_view},
};

struct Tools {
    log: Child<TextView>,
    note: Child<TextView>,
    bold: bool,
    left: bool,
    size: u32,
    said: String,
}

const TEXT: &str = "12:04:07 deskbar  warn  the tray took 14 ms to lay out\n12:04:09 tracker  info  opened /home/olo/Documents (212 files)\n12:04:11 net      error could not reach the time server: no route to host, trying again in 30 seconds\n12:04:12 deskbar  info  the tray is 3 icons wide";

/// The log's text with its times dim and its levels coloured.
fn runs(font: olo_kit::Font) -> Vec<TextRun> {
    let p = olo_kit::theme::palette();
    let ink = style::c(p.document_text);
    let run = |len, color| TextRun { len, font: font.clone(), color, background_color: None, underline: None, strikethrough: None, size: None };
    let mut out = Vec::new();
    for (i, line) in TEXT.split('\n').enumerate() {
        if i > 0 {
            out.push(run(1, ink));
        }
        let level = if line.contains(" error ") {
            hsla(0.0, 0.7, 0.5, 1.0)
        } else if line.contains(" warn ") {
            hsla(0.1, 0.8, 0.45, 1.0)
        } else {
            ink
        };
        out.push(run(8, ink.opacity(0.5)));
        out.push(run(10, ink));
        out.push(run(5, level));
        out.push(run(line.len() - 23, ink));
    }
    out
}

fn menus(v: &Tools) -> Vec<Menu> {
    vec![
        Menu::new("File").item("New", "new").key("n").item("Open…", "open").key("o").item("Save", "save").key("s").enabled(false).item("Print…", "print").key("p"),
        Menu::new("Edit").item("Undo", "undo").key("z").enabled(false).item("Redo", "redo").shift_key("z").item("Cut", "cut").key("x").item("Copy", "copy").key("c").item("Paste", "paste").key("v"),
        Menu::new("Format")
            .item("Bold", "bold")
            .key("b")
            .checked(v.bold)
            .item("Italic", "italic")
            .key("i")
            .item("Align Left", "align-left")
            .checked(v.left)
            .item("Align Center", "align-center")
            .checked(!v.left)
            .item("Picture…", "insert-picture")
            .item("Ruler", "ruler")
            .checked(true),
    ]
}

impl View for Tools {
    type Message = String;

    fn message(&mut self, command: String, cx: &mut Cx<Self>) {
        match command.as_str() {
            "bold" => self.bold = !self.bold,
            "align-left" => self.left = true,
            "align-center" => self.left = false,
            c if c.starts_with("size:") => self.size = c[5..].parse().unwrap_or(12),
            _ => {}
        }
        self.said = format!("Chose {command}.");
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let menus = menus(self);
        let chosen = std::rc::Rc::new(cx.msg_with(|c: &str| c.to_owned()));
        let on_command = move |c: &str, cx: &mut olo_kit::WindowCx| chosen(c, cx);
        let size = self.size;
        let top = ToolBar::new("tools", "Tools", on_command.clone())
            .command("new", "new-document")
            .command("open", "folder-open")
            .command("save", "save-document")
            .command("print", "printer")
            .separator()
            .command("undo", "undo")
            .command("redo", "redo")
            .separator()
            .command("cut", "cut")
            .command("copy", "copy")
            .command("paste", "paste")
            .separator()
            .toggle("bold", "format-bold")
            .toggle("italic", "format-italic")
            .separator()
            .picker("Size", format!("{size}"), 52.0, move |_| [9, 10, 12, 14, 18, 24].into_iter().fold(Menu::new("Size"), |m, s| m.item(format!("{s}"), format!("size:{s}")).checked(s == size)))
            .colors("Color", "Red", Some(hsla(0.0, 0.7, 0.5, 1.0)), |_| Menu::new("Color").item("Red", "red").item("Blue", "blue"))
            .enabled(false)
            .separator()
            .toggle("align-left", "align-left")
            .toggle("align-center", "align-center")
            .rows(2)
            .menus(&menus);
        let side = ToolBar::new("insert", "Insert", on_command).vertical().command("insert-picture", "image").separator().toggle("ruler", "ruler").menus(&menus);
        div().size_full().flex().flex_col().child(top).child(
            div().flex_1().min_h_0().flex().child(side).child(
                div()
                    .flex_1()
                    .min_w_0()
                    .p(px(m::WINDOW_INSET))
                    .flex()
                    .flex_col()
                    .gap(px(m::SPACING))
                    .child(div().flex_1().min_h_0().child(self.log.element()))
                    .child(div().h(px(90.0)).child(self.note.element()))
                    .child(StringView::new(self.said.clone()).dim(true)),
            ),
        )
    }
}

fn main() {
    olo_kit::run("olo.tools-example", |_| {
        // (TOOLS_WIDTH, TOOLS_HEIGHT: a narrower window, its tool bar
        // wrapped and the rest of its tools in its » menu; a shorter
        // one, its text views squeezed.)
        let side = |name: &str, usual: f32| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(usual);
        let (width, height) = (side("TOOLS_WIDTH", 420.0), side("TOOLS_HEIGHT", 400.0));
        olo_kit::open_window(WindowOptions::new("Tools", width, height), |cx| Tools {
            log: text_view(cx, |v| {
                let v = v.mono(true);
                let runs = runs(v.font());
                v.with_text(TEXT, Some(runs))
            }),
            note: text_view(cx, |v| v.empty_text("Select a line to read it whole.")),
            bold: true,
            left: true,
            size: 12,
            said: "Nothing yet.".into(),
        });
    });
}
