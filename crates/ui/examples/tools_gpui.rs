//! The GPUI kit's tool bars and text views in one window, laid out
//! exactly as olo-kit's `tools` example lays out its own: the two are
//! compared pixel for pixel (`tools/dev/kit-compare`).

use olo_ui::{
    Menu, WindowOptions,
    gpui::{self, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, TextRun, Window, div, hsla, px},
    metrics as m, style,
    widgets::{StringView, TextView, ToolBar, text_view},
};

struct Tools {
    log: Entity<TextView>,
    note: Entity<TextView>,
    bold: bool,
    left: bool,
    size: u32,
    said: String,
}

const TEXT: &str = "12:04:07 deskbar  warn  the tray took 14 ms to lay out\n12:04:09 tracker  info  opened /home/olo/Documents (212 files)\n12:04:11 net      error could not reach the time server: no route to host, trying again in 30 seconds\n12:04:12 deskbar  info  the tray is 3 icons wide";

fn runs(font: gpui::Font, cx: &gpui::App) -> Vec<TextRun> {
    let p = olo_ui::theme::palette(cx);
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

impl Render for Tools {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menus = menus(self);
        let me = cx.entity();
        let on_command = move |c: &str, _: &mut Window, cx: &mut gpui::App| {
            me.update(cx, |v, cx| {
                match c {
                    "bold" => v.bold = !v.bold,
                    "align-left" => v.left = true,
                    "align-center" => v.left = false,
                    c if c.starts_with("size:") => v.size = c[5..].parse().unwrap_or(12),
                    _ => {}
                }
                v.said = format!("Chose {c}.");
                cx.notify();
            })
        };
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
                    .child(div().flex_1().min_h_0().child(self.log.clone()))
                    .child(div().h(px(90.0)).child(self.note.clone()))
                    .child(StringView::new(self.said.clone()).dim(true)),
            ),
        )
    }
}

fn main() {
    olo_ui::run("olo.tools-example", |cx| {
        // (TOOLS_WIDTH, TOOLS_HEIGHT: a narrower window, its tool bar
        // wrapped and the rest of its tools in its » menu; a shorter
        // one, its text views squeezed.)
        let side = |name: &str, usual: f32| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(usual);
        let (width, height) = (side("TOOLS_WIDTH", 420.0), side("TOOLS_HEIGHT", 400.0));
        olo_ui::open_window(cx, WindowOptions::new("Tools", width, height), |_, cx| {
            cx.new(|cx| {
                let log = text_view(cx, "log", |v| v.mono(true));
                log.update(cx, |v, cx| {
                    let runs = runs(v.font(), cx);
                    v.set_text(TEXT, Some(runs), cx)
                });
                let note = text_view(cx, "note", |v| v.empty_text("Select a line to read it whole."));
                Tools { log, note, bold: true, left: true, size: 12, said: "Nothing yet.".into() }
            })
        });
    });
}
