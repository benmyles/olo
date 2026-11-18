//! A month to pick a day from (`widgets::MonthGrid`) under what steps
//! through it (`widgets::ArrowButton`): the month shown, the day chosen
//! and what's marked are the view's own; the widget says what was
//! clicked.
//!
//!     cargo build -p olo-kit --example month
//!     tsession run NAME target/debug/examples/month

use olo_kit::{
    Cx, Element, ParentElement as _, Styled as _, View, WindowOptions, div, metrics as m, px, style,
    widgets::{
        ArrowButton, MonthGrid, StringView,
        month::{self, Day},
    },
};

struct Picker {
    /// The month shown.
    shown: (i32, u8),
    chosen: Day,
    today: Day,
}

#[derive(Clone)]
enum Msg {
    Turn(i32),
    Show(i32, u8),
    Pick(Day),
}

impl View for Picker {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Turn(by) => self.shown = month::turned(self.shown.0, self.shown.1, by),
            Msg::Show(year, month) => self.shown = (year, month),
            Msg::Pick(day) => {
                self.chosen = day;
                self.shown = (day.year, day.month);
            }
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let nav = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .child(
                div().flex().flex_row().flex_shrink_0().child(ArrowButton::back("Previous Month").on_click(cx.msg(Msg::Turn(-1)))).child(ArrowButton::on("Next Month").on_click(cx.msg(Msg::Turn(1)))),
            )
            .child(div().flex_1().min_w_0().child(StringView::new(self.chosen.said()).cut(true)));
        let grid = MonthGrid::new("Month", self.shown.0, self.shown.1, self.today)
            .chosen(self.chosen, self.chosen)
            .marked(vec![self.today.plus(2), self.today.plus(3), self.today.plus(9)])
            .on_pick(cx.msg_with(|day: &Day| Msg::Pick(*day)))
            .on_turn(cx.msg_with(|(year, month): &(i32, u8)| Msg::Show(*year, *month)));
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .bg(style::c(olo_kit::theme::palette().panel_background))
            .child(nav)
            .child(div().flex().justify_center().child(grid))
    }
}

fn main() {
    olo_kit::run("olo.month-example", |_| {
        let today = Day::new(2026, 11, 18);
        olo_kit::open_window(WindowOptions::new("Month", 2.0 * m::WINDOW_INSET + 280.0, 0.0).fits_content(), move |_| Picker { shown: (today.year, today.month), chosen: today, today });
    });
}
