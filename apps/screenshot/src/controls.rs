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

use olo_kit::{
    Bounds, BoxShadow, ClickEvent, Corners, Cx, Element, FontWeight, LayerKeyboard, LayerShellLayer, ParentElement as _, Styled as _, View, WindowOptions, canvas, div, hsla, metrics as m, point, px,
    size, style, theme,
    widgets::{Button, SMALL_BUTTON_HEIGHT, StringView, button_width},
};
use olo_protocol::recording::{CONTROLS_NAMESPACE, Going, going};

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
    /// The clock's tick (the one numbered so: an older one is dropped).
    Tick(u64),
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
    /// The clock's tick that counts (one asked for since isn't waited
    /// for twice).
    tick: u64,
    _watch: Option<olo_kit::watch::Watch>,
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
    /// The clock's next tick (on the second, as it counts), while it
    /// records; none while paused or counting down.
    fn schedule(&mut self, cx: &mut Cx<Self>) {
        self.tick += 1;
        if !self.going.recording || self.going.paused {
            return;
        }
        let next = 1000 - self.going.recorded_now() % 1000;
        cx.after(Duration::from_millis(next + 5), Msg::Tick(self.tick));
    }

    /// Run `act` (it waits for the recorder) off the window's looper.
    fn act(&mut self, busy: Option<Busy>, act: fn(), cx: &mut Cx<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = busy;
        cx.invalidate();
        cx.work(act, |()| Msg::Done);
    }
}

impl View for Controls {
    type Message = Msg;

    fn message(&mut self, m: Msg, cx: &mut Cx<Self>) {
        match m {
            Msg::Changed | Msg::Done => match going() {
                Some(g) => {
                    self.going = g;
                    self.schedule(cx);
                    cx.invalidate();
                }
                // The recording ended (stopped, discarded): so do we.
                None => olo_kit::quit(),
            },
            Msg::Tick(n) if n == self.tick => {
                self.schedule(cx);
                cx.invalidate();
            }
            Msg::Tick(_) => {}
        }
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
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
        let time_w = (f32::from(style::text_width(widest(ms), style::PLAIN_SIZE, font)) / 2.0).ceil() * 2.0;
        let saying = busy.is_some() || !recording;
        let pause = if paused { "Resume" } else { "Pause" };
        // The bar's size: its parts side by side, `TIGHT` apart and from
        // its edges, as a tool bar's (Pause and Resume given the same
        // room); the window round it has room for its shadow.
        let pause_w = button_width("Pause", true).max(button_width("Resume", true));
        let rest = ["Mark", "Restart", "Discard", "Stop"];
        let buttons: f32 = pause_w + rest.iter().map(|l| button_width(l, true)).sum::<f32>();
        let bar_w = m::TIGHT + LIGHT + m::TIGHT + time_w + m::TIGHT + buttons + m::TIGHT * rest.len() as f32 + m::TIGHT;
        let bar_h = SMALL_BUTTON_HEIGHT + 2.0 * m::TIGHT;
        let (w, h) = (bar_w + 2.0 * SHADOW, bar_h + 2.0 * SHADOW);
        if (w, h) != self.size {
            self.size = (w, h);
            cx.resize_window(size(px(w), px(h)));
        }
        let on = |busy: Option<Busy>, act: fn()| cx.listener(move |c: &mut Controls, _: &ClickEvent, cx| c.act(busy, act, cx));
        let toggle_pause: fn() = if paused {
            || {
                let _ = recording::pause(false);
            }
        } else {
            || {
                let _ = recording::pause(true);
            }
        };
        let back = canvas(move |g, b| {
            let p = theme::palette();
            let bar = Bounds::new(b.origin + point(px(SHADOW), px(SHADOW * 0.6)), size(px(bar_w), px(bar_h)));
            g.paint_shadows(bar, Corners::default(), &[BoxShadow { color: hsla(0.0, 0.0, 0.0, 0.3), offset: point(px(0.0), px(2.0)), blur_radius: px(SHADOW * 0.75), spread_radius: px(0.0) }]);
            style::fill_rect(g, bar, style::c(p.panel_background));
            style::raised(g, bar, &p);
            style::frame_rect(g, bar, style::tint(p.panel_background, 1.6));
            let light = Bounds::new(point(bar.left() + px(m::TIGHT), bar.top() + (bar.size.height - px(LIGHT)) / 2.0), size(px(LIGHT), px(LIGHT)));
            style::recording_light(g, light, &p, paused);
            olo_kit::agent::record(
                g,
                olo_kit::agent::Node { role: "Image", name: Some(if paused { "Recording, paused" } else { "Recording" }.into()), bounds: light, enabled: true, ..Default::default() },
            );
        })
        .paint_key(style::key((paused, bar_w.to_bits(), bar_h.to_bits())))
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .size_full();
        let mut row = div().absolute().left(px(SHADOW)).top(px(SHADOW * 0.6)).w(px(bar_w)).h(px(bar_h)).flex().items_center().pl(px(m::TIGHT + LIGHT + m::TIGHT)).gap(px(m::TIGHT));
        if saying {
            row = row.child(StringView::new(time_text));
        } else {
            row = row
                .child(div().w(px(time_w)).child(StringView::new(time_text)))
                .child(Button::new(pause).id("pause").small().min_width(pause_w).on_click(on(None, toggle_pause)))
                .child(Button::new("Mark").small().on_click(on(None, || {
                    let _ = recording::mark(&["--mark".to_owned(), String::new()]);
                })))
                .child(Button::new("Restart").small().on_click(on(Some(Busy::Restarting), restart)))
                .child(Button::new("Discard").small().on_click(on(Some(Busy::Discarding), || {
                    let _ = recording::discard();
                })))
                .child(Button::new("Stop").small().default(true).on_click(on(Some(Busy::Stopping), || {
                    let _ = recording::stop();
                })));
        }
        div().size_full().relative().child(back).child(row)
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
        let _watch = olo_kit::watch::watch([path.clone()], move |_| {
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
    olo_kit::run(APP_ID, move |_| {
        let options = WindowOptions::new("Recording", 420.0, SMALL_BUTTON_HEIGHT + 2.0 * m::TIGHT)
            .no_panel()
            .layer(CONTROLS_NAMESPACE, LayerShellLayer::Overlay, (true, false, false, false))
            .layer_options(|l| {
                // (The bar `MARGIN` from the top: its shadow's room above it.)
                l.margin = ((MARGIN - SHADOW * 0.6).round() as i32, 0, 0, 0);
                l.keyboard = LayerKeyboard::None;
            });
        olo_kit::open_window(options, move |cx| {
            let to = cx.messenger();
            let watch = olo_kit::watch::watch([path], move |_| {
                to.send(Msg::Changed);
            });
            let mut c = Controls { going: first, busy: None, size: (0.0, 0.0), tick: 0, _watch: watch };
            c.schedule(cx);
            c
        });
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
