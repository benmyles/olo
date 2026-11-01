//! Tooltips: a line or two of text in the palette's tooltip colours (a
//! pale yellow in light mode, as BeOS's), shown by GPUI over the element
//! after the pointer rests on it (`.id(..).tooltip(widgets::tooltip(text))`).

use gpui::{AnyView, App, AppContext as _, Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div, px};
use olo_kit_base::metrics as m;

use crate::{style, theme};

struct Tooltip {
    text: SharedString,
}

impl Render for Tooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        div()
            .max_w(px(360.0))
            .px(px(m::TIGHT + 1.0))
            .py(px(2.0))
            .bg(style::c(p.tooltip_background))
            .border(style::hairline(window))
            .border_color(style::shade(&p, p.tooltip_background, 1.4))
            .text_color(style::c(p.tooltip_text))
            .child(self.text.clone())
    }
}

/// What `.tooltip` takes: a tooltip saying `text`.
pub fn tooltip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| Tooltip { text: text.clone() }).into()
}
