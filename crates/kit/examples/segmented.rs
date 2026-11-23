//! The kit's segmented control in each of its forms, in one window: to
//! picture it in every look and at every scale.

use olo_kit::{
    Cx, Element, ParentElement as _, Styled as _, View, WindowOptions, div, metrics as m, px,
    widgets::{Mark, Segment, SegmentedControl, StringView, heading, separator},
};

struct Segments {
    /// What each control shown has chosen.
    chosen: [usize; 6],
}

#[derive(Clone)]
enum Msg {
    Choose(usize, usize),
}

impl View for Segments {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        let Msg::Choose(which, i) = message;
        self.chosen[which] = i;
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let row = |label: &'static str, control: SegmentedControl| div().flex().items_center().gap(px(m::SPACING)).child(div().w(px(120.0)).child(StringView::new(label).dim(true))).child(control);
        let side = |waiting: usize| vec![Segment::new("Sessions").mark(Mark::Terminal), Segment::new("Activity").mark(Mark::Pulse).count(waiting)];
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(heading("Segmented control"))
            .child(row("Marks and a count", SegmentedControl::new("a", side(3), self.chosen[0]).on_choose(cx.msg_with(|i: &usize| Msg::Choose(0, *i)))))
            .child(row("The other chosen", SegmentedControl::new("b", side(3), 1 - self.chosen[0].min(1)).on_choose(cx.msg_with(|i: &usize| Msg::Choose(1, *i)))))
            .child(row("In a 224-point sidebar", SegmentedControl::new("c", side(2), self.chosen[2]).width(200.0).on_choose(cx.msg_with(|i: &usize| Msg::Choose(2, *i)))))
            .child(row("In a 204-point one", SegmentedControl::new("c2", side(12), self.chosen[2]).width(180.0).on_choose(cx.msg_with(|i: &usize| Msg::Choose(2, *i)))))
            .child(row("Names only", SegmentedControl::new("d", [Segment::new("Messages").count(2), Segment::new("Contacts")], self.chosen[3]).on_choose(cx.msg_with(|i: &usize| Msg::Choose(3, *i)))))
            .child(separator())
            .child(row("Three", SegmentedControl::new("e", ["Day", "Week", "Month"], self.chosen[4]).on_choose(cx.msg_with(|i: &usize| Msg::Choose(4, *i)))))
            .child(row("Three, the middle", SegmentedControl::new("f", ["Day", "Week", "Month"], 1)))
            .child(row("Four", SegmentedControl::new("g", ["Free", "16:9", "4:3", "1:1"], self.chosen[5]).on_choose(cx.msg_with(|i: &usize| Msg::Choose(5, *i)))))
            .child(row("Not enabled", SegmentedControl::new("h", side(3), 0).enabled(false)))
    }
}

fn main() {
    olo_kit::run("olo.segmented", |_| {
        olo_kit::open_window(WindowOptions::new("Segmented Control", 500.0, 396.0), |_| Segments { chosen: [0, 1, 1, 0, 0, 0] });
    });
}
