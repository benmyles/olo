//! The controls on the screen while a person records
//! (`olo-screenshot --controls`, which `--toggle-record` starts beside
//! the recorder): a small bar at the top of the screen, as CleanShot X's,
//! with the record light, how long it has recorded (pauses left out),
//! and Pause (Resume), Mark, Restart, Discard and Stop. It is a layer
//! surface in `CONTROLS_NAMESPACE`, which app_server leaves out of every
//! capture: on the screen, never in the recording or a share. Never takes
//! the keyboard (what's recorded keeps it); an agent has the same through
//! `olo-screenshot --pause`, `--mark`, `--discard`, `--stop`.
//!
//! It follows the recorder's status file (`olo_protocol::recording`,
//! watched, never polled): paused or going, and its clock (`recorded_ms`
//! as of `as_of`), which ticks here once a second while it records and
//! not at all while paused. It goes when the recording does.

use std::time::Duration;

use olo_protocol::recording::{CONTROLS_NAMESPACE, Going, going};
use olo_ui::{
    WindowOptions,
    gpui::{
        self, AppContext as _, Context, FontWeight, IntoElement, LayerKeyboard, LayerShellLayer, ParentElement as _, Render, Styled as _, Task, Window, canvas, div, prelude::FluentBuilder as _, px,
        size,
    },
    metrics as m, style, theme,
    widgets::{
        StringView,
        buttons::{Button, SMALL_BUTTON_HEIGHT, button_width},
    },
};

use crate::recording;

const APP_ID: &str = "olo.screenshot-controls";
/// From the top of the screen to the bar.
const MARGIN: f32 = 8.0;
/// Room round the bar for its shadow (points; less above, where the
/// light comes from).
const SHADOW: f32 = 10.0;
/// The record light's square.
const LIGHT: f32 = 14.0;

enum Msg {
    /// The status file changed.
    Changed,
    /// An action finished (the recorder told): the status says the rest.
    Done,
}

/// What the person asked for, while the recorder does it.
#[derive(Clone, Copy, PartialEq)]
enum Busy {
    Stopping,
    Discarding,
    Restarting,
}

struct Controls {
    going: Going,
    busy: Option<Busy>,
    /// The bar's size as last laid out (points), to fit the window to it.
    size: (f32, f32),
    /// The next tick of the clock, while it records.
    tick: Option<Task<()>>,
    _watch: Option<olo_ui::watch::Watch>,
}

/// `m:ss`, or `h:mm:ss` past an hour.
fn clock(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// The widest the clock gets (its digits are tabular): room kept for it,
/// so the bar doesn't change size as it counts.
fn widest(ms: u64) -> &'static str {
    if ms >= 36_000_000 {
        "00:00:00"
    } else if ms >= 3_600_000 {
        "0:00:00"
    } else {
        "00:00"
    }
}

impl Controls {
    fn message(&mut self, m: Msg, cx: &mut Context<Self>) {
        match m {
            Msg::Changed | Msg::Done => match going() {
                Some(g) => {
                    self.going = g;
                    self.schedule(cx);
                    cx.notify();
                }
                // The recording ended (stopped, discarded): so do we.
                None => cx.quit(),
            },
        }
    }

    /// The clock's next tick (on the second, as it counts), while it
    /// records; none while paused or counting down.
    fn schedule(&mut self, cx: &mut Context<Self>) {
        self.tick = None;
        if !self.going.recording || self.going.paused {
            return;
        }
        let next = 1000 - self.going.recorded_now() % 1000;
        self.tick = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(next + 5)).await;
            let _ = this.update(cx, |c, cx| {
                c.schedule(cx);
                cx.notify();
            });
        }));
    }

    /// Run `act` (it waits for the recorder) off the UI thread.
    fn act(&mut self, busy: Option<Busy>, act: impl FnOnce() + Send + 'static, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = busy;
        cx.notify();
        let proxy = olo_ui::proxy(cx);
        std::thread::spawn(move || {
            act();
            proxy.send(Msg::Done);
        });
    }
}

impl Render for Controls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let (recording, paused, busy) = (self.going.recording, self.going.paused, self.busy);
        let ms = self.going.recorded_now();
        let time_text = match busy {
            Some(Busy::Stopping) => "Saving…".to_owned(),
            Some(Busy::Discarding) => "Discarding…".to_owned(),
            Some(Busy::Restarting) => "Restarting…".to_owned(),
            None if recording => clock(ms),
            None => "Starting…".to_owned(),
        };
        // Room for the clock at its widest (its digits are tabular): it
        // doesn't move the buttons as it counts. What's being done (or
        // the countdown) takes the whole row instead, the buttons gone.
        let font = style::ui_font(FontWeight::NORMAL);
        let time_w = (f32::from(style::text_width(window, widest(ms), style::PLAIN_SIZE, font)) / 2.0).ceil() * 2.0;
        let saying = busy.is_some() || !recording;
        let pause = if paused { "Resume" } else { "Pause" };
        // The bar's size: its parts side by side, `TIGHT` apart and from
        // its edges, as a tool bar's (Pause and Resume given the same
        // room); the window round it has room for its shadow.
        let pause_w = button_width(window, "Pause", true).max(button_width(window, "Resume", true));
        let rest = ["Mark", "Restart", "Discard", "Stop"];
        let buttons: f32 = pause_w + rest.iter().map(|l| button_width(window, l, true)).sum::<f32>();
        let bar_w = m::TIGHT + LIGHT + m::TIGHT + time_w + m::TIGHT + buttons + m::TIGHT * rest.len() as f32 + m::TIGHT;
        let bar_h = SMALL_BUTTON_HEIGHT + 2.0 * m::TIGHT;
        let (w, h) = (bar_w + 2.0 * SHADOW, bar_h + 2.0 * SHADOW);
        if (w, h) != self.size {
            self.size = (w, h);
            window.defer(cx, move |window, _| window.resize(size(px(w), px(h))));
        }
        let me = cx.entity();
        let on = |busy: Option<Busy>, act: fn()| {
            let me = me.clone();
            move |_: &mut Window, cx: &mut gpui::App| me.update(cx, |c, cx| c.act(busy, act, cx))
        };
        let toggle_pause: fn() = if paused {
            || {
                let _ = recording::pause(false);
            }
        } else {
            || {
                let _ = recording::pause(true);
            }
        };
        let panel = style::c(p.panel_background);
        div()
            .size_full()
            .relative()
            .child(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, cx| {
                        let p = theme::palette(cx);
                        let bar = gpui::Bounds::new(b.origin + gpui::point(px(SHADOW), px(SHADOW * 0.6)), size(px(bar_w), px(bar_h)));
                        window.paint_shadows(
                            bar,
                            gpui::Corners::default(),
                            &[gpui::BoxShadow { color: gpui::hsla(0.0, 0.0, 0.0, 0.3), offset: gpui::point(px(0.0), px(2.0)), blur_radius: px(SHADOW * 0.75), spread_radius: px(0.0) }],
                        );
                        style::fill_rect(window, bar, panel);
                        style::raised(window, bar, &p);
                        style::frame_rect(window, bar, style::tint(p.panel_background, 1.6));
                        let light = gpui::Bounds::new(gpui::point(bar.left() + px(m::TIGHT), bar.top() + (bar.size.height - px(LIGHT)) / 2.0), size(px(LIGHT), px(LIGHT)));
                        style::recording_light(window, light, &p, paused);
                        olo_ui::agent::record(
                            window,
                            cx,
                            olo_ui::agent::Node { role: "Image", name: Some(if paused { "Recording, paused" } else { "Recording" }.into()), bounds: light, enabled: true, ..Default::default() },
                        );
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left(px(SHADOW))
                    .top(px(SHADOW * 0.6))
                    .w(px(bar_w))
                    .h(px(bar_h))
                    .flex()
                    .items_center()
                    .pl(px(m::TIGHT + LIGHT + m::TIGHT))
                    .gap(px(m::TIGHT))
                    .when(saying, |row| row.child(StringView::new(time_text.clone())))
                    .when(!saying, |row| row.child(div().w(px(time_w)).child(StringView::new(time_text))))
                    .when(!saying, |row| {
                        row.child(Button::new(pause).id("pause").small().min_width(pause_w).on_click(on(None, toggle_pause)))
                            .child(Button::new("Mark").small().on_click(on(None, || {
                                let _ = recording::mark(&["--mark".to_owned(), String::new()]);
                            })))
                            .child(Button::new("Restart").small().on_click(on(Some(Busy::Restarting), restart)))
                            .child(Button::new("Discard").small().on_click(on(Some(Busy::Discarding), || {
                                let _ = recording::discard();
                            })))
                            .child(Button::new("Stop").small().default(true).on_click(on(Some(Busy::Stopping), || {
                                let _ = recording::stop();
                            })))
                    }),
            )
    }
}

/// Restart (`recording::restart`), its answer left unsaid.
fn restart() {
    let _ = recording::restart();
}

/// `--controls`: shown while the recording on this display goes.
pub fn run() -> std::process::ExitCode {
    // The recorder counts down first: its status comes when it's ready.
    let path = olo_protocol::recording::path_for(&olo_protocol::recording::this_display());
    let first = {
        let (tx, rx) = std::sync::mpsc::channel();
        let _watch = olo_ui::watch::watch([path.clone()], move |_| {
            let _ = tx.send(());
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(g) = going() {
                break Some(g);
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() || rx.recv_timeout(left).is_err() {
                break None;
            }
        }
    };
    let Some(first) = first else { return std::process::ExitCode::SUCCESS };
    olo_ui::run(APP_ID, move |cx| {
        let options = WindowOptions::new("Recording", 420.0, SMALL_BUTTON_HEIGHT + 2.0 * m::TIGHT)
            .no_panel()
            .layer(CONTROLS_NAMESPACE, LayerShellLayer::Overlay, (true, false, false, false))
            .layer_options(|l| {
                // (The bar `MARGIN` from the top: its shadow's room above it.)
                l.margin = ((MARGIN - SHADOW * 0.6).round() as i32, 0, 0, 0);
                l.keyboard = LayerKeyboard::None;
            });
        let proxy = olo_ui::proxy(cx);
        let watch = olo_ui::watch::watch([path], move |_| proxy.send(Msg::Changed));
        let controls = cx.new(|cx| {
            let mut c = Controls { going: first, busy: None, size: (0.0, 0.0), tick: None, _watch: watch };
            c.schedule(cx);
            c
        });
        let messages = controls.clone();
        olo_ui::message_handler(cx, move |m, cx| {
            if let Ok(m) = m.downcast::<Msg>() {
                messages.update(cx, |c, cx| c.message(*m, cx));
            }
        });
        let view = controls.clone();
        olo_ui::open_window(cx, options, move |_, _| view);
    });
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_reads_as_a_stopwatch() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(61_999), "1:01");
        assert_eq!(clock(3_725_000), "1:02:05");
        assert_eq!(widest(59_000), "00:00");
        assert_eq!(widest(3_600_000), "0:00:00");
    }
}
