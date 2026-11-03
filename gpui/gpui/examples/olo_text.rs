//! Olo's text, for looking at closely: the kit's sizes and faces, light
//! on dark and dark on light, with symbols, CJK and emoji.

use gpui::{
    App, Application, Bounds, Context, FontWeight, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, rgb, size,
};

struct Sample;

const LINE: &str = "Hamburgefonstiv 0123456789 — The quick brown fox jumps.";

impl Render for Sample {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let block =
            |bg: u32, fg: u32| {
                let mut d = div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .bg(rgb(bg))
                    .text_color(rgb(fg))
                    .font_family("Noto Sans");
                for size in [10.5, 12.0, 13.0, 16.0, 24.0] {
                    d = d.child(div().text_size(px(size)).child(format!("{size} {LINE}")));
                }
                d.child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .child("Bold 12: File Edit View Window Help"),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .italic()
                        .child("Italic 12: an italic line of text"),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_family("Noto Sans Mono")
                        .child("Mono 12: fn main() { println!(\"{}\", 42); } ═══"),
                )
                .child(div().text_size(px(12.0)).child(
                    "Symbols: ⌘ ⌥ ⇧ ⌃ ▸ ✓ → ← ⏎ ⌫  CJK: 日本語 中文 한국어  Emoji: 😀 👍🏽 🎉 ☺️",
                ))
            };
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(block(0xffffff, 0x000000))
            .child(block(0x1e1e1e, 0xe8e8e8))
            .child(
                div()
                    .flex()
                    .gap_4()
                    .p_3()
                    .bg(rgb(0xd5d8dd))
                    .text_color(rgb(0x1f2227))
                    .font_family("Noto Sans")
                    .text_size(px(12.0))
                    .child("Appearance")
                    .child("Backgrounds")
                    .child("Integrations")
                    .child("Keyboard")
                    .child("Touchpad & Mouse"),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(620.), px(640.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some("olo.text-sample".into()),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Sample),
        )
        .unwrap();
    });
}
