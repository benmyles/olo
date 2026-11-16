//! `Button::working`: a button showing the long job it started on itself,
//! as far as it's got (and a job whose progress isn't known, its band
//! sweeping), beside the same button at rest; and a pretend job of six
//! seconds, Start pressed, its button filling as it goes.

use std::time::{Duration, Instant};

use olo_kit::{Cx, Element, IntoElement as _, ParentElement as _, Styled as _, View, WindowOptions, div, metrics as m, px, widgets::Button};

struct Working {
    started: Option<Instant>,
}

#[derive(Clone)]
enum Msg {
    Start,
    Tick,
}

/// The pretend job's length.
const JOB: Duration = Duration::from_secs(6);

impl View for Working {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Start => self.started = Some(Instant::now()),
            Msg::Tick => {}
        }
        if self.started.is_some_and(|s| s.elapsed() >= JOB) {
            self.started = None;
        }
        if self.started.is_some() {
            // (A tenth of a second a step, as a job tells how it goes.)
            cx.after(Duration::from_millis(100), Msg::Tick);
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let job = match self.started {
            Some(s) => Button::new("Start").id("job").working(Some(s.elapsed().as_secs_f32() / JOB.as_secs_f32())),
            None => Button::new("Start").id("job").on_click(cx.msg(Msg::Start)),
        };
        let row = |b: Button| div().flex().gap(px(m::SPACING)).child(b);
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(row(Button::new("Export Video…")).child(Button::new("Export Video…").id("at-0").working(Some(0.0))).child(Button::new("Export Video…").id("at-42").working(Some(0.42))))
            .child(
                row(Button::new("Install Updates").id("at-100").working(Some(1.0)))
                    .child(Button::new("Install Updates").id("unknown").working(None))
                    .child(Button::new("Download").default(true).working(Some(0.7))),
            )
            .child(row(job))
            .into_element()
    }
}

fn main() {
    olo_kit::run("olo.kit.working", |_| {
        olo_kit::open_window(WindowOptions::new("Working", 480.0, 160.0).fits_content(), |_| Working { started: None });
    });
}
