//! What the person feels: how long a key or a click takes to change the
//! screen, and where the time went, so that "typing lags" has a figure
//! and a culprit the moment it happens (Ben, 2026-11-20: "something is
//! really broken my desktop is lagging"; that day it took a day's
//! tracing to say the compositor was on time and the kernel's own
//! threads weren't).
//!
//! One key press or button press at a time is followed, from the three
//! places app_server already stands at:
//!
//! - **to here**: from the device's own stamp on the event to app_server
//!   handing it to the program (the kernel, the input thread, the loop);
//! - **to draw**: until the program next hands in a drawing (its commit:
//!   the program's own work, and whatever kept it from a processor);
//! - **to show**: until the frame with that drawing is on the display
//!   (app_server's frame and the wait for the display's next refresh).
//!
//! The middle one is a guess where the program draws for another reason
//! first (a caret's blink after a key it did nothing with), so a drawing
//! that comes later than a second isn't taken for the answer, and keys
//! that change nothing on their own (the modifiers) aren't followed.
//!
//! One late in all ([`LATE`]) is said in the log with its three parts
//! and what the computer was short of, at most once every few seconds
//! ([`SAID_EVERY`]). The last ones are kept ([`KEPT`]) for
//! `Request::Timings`. A press costs a few stores; a commit while none
//! is followed, one comparison; nothing runs otherwise.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// A key or a click, from the device to the screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Taken {
    pub what: What,
    /// The program's app id.
    pub app: String,
    /// None where the event carried no stamp of the device's (made up
    /// by a tool, or a clock that isn't the one read here).
    pub to_here: Option<Duration>,
    pub to_draw: Duration,
    pub to_show: Duration,
}

impl Taken {
    pub fn in_all(&self) -> Duration {
        self.to_here.unwrap_or_default() + self.to_draw + self.to_show
    }

    /// "a key in olo-terminal took 143 ms to show: 2 ms to reach it,
    /// 128 ms until it drew, 13 ms to the display".
    pub fn said(&self) -> String {
        let ms = |d: Duration| format!("{:.0} ms", d.as_secs_f64() * 1000.0);
        let here = self.to_here.map(|d| format!("{} to reach it, ", ms(d))).unwrap_or_default();
        format!("{} in {} took {} to show: {here}{} until it drew, {} to the display", self.what.word(), self.app, ms(self.in_all()), ms(self.to_draw), ms(self.to_show))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    Key,
    Click,
}

impl What {
    fn word(self) -> &'static str {
        match self {
            What::Key => "a key",
            What::Click => "a click",
        }
    }
}

/// A drawing later than this after the press isn't taken for its answer.
pub const ANSWER: Duration = Duration::from_secs(1);
/// From the device to the display, this long is said in the log: six
/// frames at 60 Hz, twelve at 120, where a person begins to feel it.
pub const LATE: Duration = Duration::from_millis(100);
/// How often at the most a late one is said.
pub const SAID_EVERY: Duration = Duration::from_secs(5);
/// How many of the last are kept.
pub const KEPT: usize = 128;

#[derive(Debug)]
struct Followed<C> {
    what: What,
    app: String,
    client: C,
    to_here: Option<Duration>,
    at: Instant,
    drew: Option<Instant>,
    /// The display whose frame has its drawing in it, once one is drawn.
    on: Option<String>,
}

/// `C`: what a program is known by (its Wayland client).
#[derive(Debug)]
pub struct Felt<C> {
    followed: Option<Followed<C>>,
    kept: VecDeque<Taken>,
    said: Option<Instant>,
    /// Late ones since the last said.
    unsaid: u32,
}

impl<C> Default for Felt<C> {
    fn default() -> Self {
        Felt { followed: None, kept: VecDeque::new(), said: None, unsaid: 0 }
    }
}

impl<C: PartialEq> Felt<C> {
    /// A key or a button was pressed and handed to `client`, `to_here`
    /// after the device stamped it. Followed unless the last one is
    /// still on its way to the display (one at a time: a sample of a
    /// burst of typing, not every key of it).
    pub fn pressed(&mut self, what: What, client: C, app: impl FnOnce() -> String, to_here: Option<Duration>, now: Instant) {
        if !self.would_follow(now) {
            return;
        }
        self.followed = Some(Followed { what, app: app(), client, to_here, at: now, drew: None, on: None });
    }

    /// Whether a press now would be followed (asked before its program
    /// is looked up): not while the last one is on its way to the
    /// display, unless that one has been for two seconds (a window that
    /// never shows).
    pub fn would_follow(&self, now: Instant) -> bool {
        !self.followed.as_ref().is_some_and(|f| f.drew.is_some() && now.duration_since(f.at) < ANSWER * 2)
    }

    /// Whether a program's next drawing is waited for (asked before
    /// anything else is found out about a commit).
    pub fn awaits_drawing(&self) -> bool {
        self.followed.as_ref().is_some_and(|f| f.drew.is_none())
    }

    /// `client` handed in a drawing.
    pub fn drew(&mut self, client: &C, now: Instant) {
        let Some(f) = self.followed.as_mut().filter(|f| f.drew.is_none() && f.client == *client) else { return };
        if now.duration_since(f.at) > ANSWER {
            // (Not this press's answer: something else it drew.)
            self.followed = None;
        } else {
            f.drew = Some(now);
        }
    }

    /// Whether a frame with the followed drawing in it is still to be
    /// drawn (asked as a display's frame is drawn).
    pub fn awaits_frame(&self) -> bool {
        self.followed.as_ref().is_some_and(|f| f.drew.is_some() && f.on.is_none())
    }

    /// The program whose drawing is followed.
    pub fn followed(&self) -> Option<&C> {
        self.followed.as_ref().map(|f| &f.client)
    }

    /// A frame of `output`, which shows the program's window, was drawn
    /// with what it has handed in.
    pub fn framed(&mut self, output: &str) {
        if let Some(f) = self.followed.as_mut().filter(|f| f.drew.is_some() && f.on.is_none()) {
            f.on = Some(output.to_owned());
        }
    }

    /// Whether a frame with the followed drawing in it is on its way to
    /// a display (asked at every display's every refresh, before the
    /// display is so much as named).
    pub fn awaits_show(&self) -> bool {
        self.followed.as_ref().is_some_and(|f| f.on.is_some())
    }

    /// `output`'s frame is on the display: the followed press, if this
    /// was its frame, with whether it is to be said in the log now.
    pub fn shown(&mut self, output: &str, now: Instant) -> Option<(Taken, Option<u32>)> {
        if self.followed.as_ref().and_then(|f| f.on.as_deref()) != Some(output) {
            return None;
        }
        let f = self.followed.take()?;
        let drew = f.drew?;
        let taken = Taken { what: f.what, app: f.app, to_here: f.to_here, to_draw: drew.duration_since(f.at), to_show: now.duration_since(drew) };
        if self.kept.len() == KEPT {
            self.kept.pop_front();
        }
        self.kept.push_back(taken.clone());
        let say = if taken.in_all() < LATE {
            None
        } else if self.said.is_some_and(|at| now.duration_since(at) < SAID_EVERY) {
            self.unsaid += 1;
            None
        } else {
            self.said = Some(now);
            Some(std::mem::take(&mut self.unsaid))
        };
        Some((taken, say))
    }

    /// The last ones, oldest first.
    pub fn kept(&self) -> impl Iterator<Item = &Taken> {
        self.kept.iter()
    }
}

/// How long ago the device stamped an event, from its stamp and the same
/// clock read now (both microseconds of the monotonic clock): None where
/// the stamp is ahead of now or far behind it (another clock, a made-up
/// event).
pub fn since_stamp(stamp_us: u64, now_us: u64) -> Option<Duration> {
    let behind = now_us.checked_sub(stamp_us)?;
    (behind < 5_000_000).then(|| Duration::from_micros(behind))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A key to the program, its drawing, a frame with it, the display:
    /// its three parts; a quick one isn't said, a late one is, and
    /// others late within a few seconds are counted for the next saying.
    #[test]
    fn a_press_is_followed_to_the_display() {
        let t0 = Instant::now();
        let mut felt: Felt<u32> = Felt::default();
        assert!(!felt.awaits_drawing() && !felt.awaits_frame());
        felt.pressed(What::Key, 7, || "olo-terminal".into(), Some(ms(1)), t0);
        assert!(felt.awaits_drawing());
        // Another program's drawing isn't its answer.
        felt.drew(&8, t0 + ms(2));
        assert!(felt.awaits_drawing());
        felt.drew(&7, t0 + ms(4));
        assert!(!felt.awaits_drawing() && felt.awaits_frame());
        // A frame of another display coming on changes nothing.
        assert_eq!(felt.shown("eDP-1", t0 + ms(5)), None);
        felt.framed("DP-2");
        assert!(!felt.awaits_frame());
        assert_eq!(felt.shown("eDP-1", t0 + ms(6)), None);
        let (taken, say) = felt.shown("DP-2", t0 + ms(12)).unwrap();
        assert_eq!(taken, Taken { what: What::Key, app: "olo-terminal".into(), to_here: Some(ms(1)), to_draw: ms(4), to_show: ms(8) });
        assert_eq!((taken.in_all(), say), (ms(13), None));
        assert_eq!(felt.shown("DP-2", t0 + ms(20)), None);

        // A late one, said; the next late ones within five seconds
        // counted, and told with the one said after that.
        let mut at = t0 + ms(100);
        let mut late = |felt: &mut Felt<u32>, at: Instant| {
            felt.pressed(What::Click, 7, || "tracker".into(), None, at);
            felt.drew(&7, at + ms(150));
            felt.framed("DP-2");
            felt.shown("DP-2", at + ms(160)).unwrap()
        };
        let (taken, say) = late(&mut felt, at);
        assert_eq!(say, Some(0));
        assert_eq!(taken.said(), "a click in tracker took 160 ms to show: 150 ms until it drew, 10 ms to the display");
        for _ in 0..3 {
            at += ms(1000);
            assert_eq!(late(&mut felt, at).1, None);
        }
        at += ms(5000);
        assert_eq!(late(&mut felt, at).1, Some(3));
        assert_eq!(felt.kept().count(), 6);
    }

    /// What isn't an answer isn't counted: a drawing over a second after
    /// the press (the program did nothing with the key; this is its
    /// caret's blink); and while one is on its way to the display the
    /// next press isn't followed, but one stuck there for good gives way.
    #[test]
    fn a_drawing_long_after_is_no_answer() {
        let t0 = Instant::now();
        let mut felt: Felt<u32> = Felt::default();
        felt.pressed(What::Key, 7, || "a".into(), None, t0);
        felt.drew(&7, t0 + ms(1200));
        assert!(!felt.awaits_drawing() && !felt.awaits_frame());
        assert_eq!(felt.kept().count(), 0);

        felt.pressed(What::Key, 7, || "first".into(), None, t0);
        felt.drew(&7, t0 + ms(3));
        // On its way: the next press isn't followed in its place.
        felt.pressed(What::Key, 7, || "second".into(), None, t0 + ms(5));
        felt.framed("DP-2");
        assert_eq!(felt.shown("DP-2", t0 + ms(9)).unwrap().0.app, "first");
        // A window that never shows (hidden, on a display that is off):
        // its press gives way to a later one.
        felt.pressed(What::Key, 7, || "hidden".into(), None, t0 + ms(20));
        felt.drew(&7, t0 + ms(22));
        felt.pressed(What::Key, 7, || "later".into(), None, t0 + ms(3000));
        felt.drew(&7, t0 + ms(3004));
        felt.framed("DP-2");
        assert_eq!(felt.shown("DP-2", t0 + ms(3010)).unwrap().0.app, "later");
    }

    #[test]
    fn a_devices_stamp_is_believed_when_it_is_just_behind() {
        assert_eq!(since_stamp(1_000_000, 1_002_500), Some(Duration::from_micros(2500)));
        assert_eq!(since_stamp(1_002_500, 1_000_000), None);
        assert_eq!(since_stamp(0, 60_000_000), None);
    }

    /// What it costs (`cargo test --release -p app_server felt_costs --
    /// --ignored --nocapture`): the question every commit asks while
    /// nothing is followed, and a whole press followed to the display.
    #[test]
    #[ignore = "a measurement"]
    fn what_following_costs() {
        const TIMES: u32 = 5_000_000;
        let felt: Felt<u64> = Felt::default();
        let started = Instant::now();
        let mut waiting = 0u32;
        for _ in 0..TIMES {
            waiting += std::hint::black_box(&felt).awaits_drawing() as u32;
        }
        println!("a commit with nothing followed: {:.2} ns ({waiting})", started.elapsed().as_secs_f64() * 1e9 / f64::from(TIMES));
        let mut felt: Felt<u64> = Felt::default();
        let t0 = Instant::now();
        let started = Instant::now();
        const PRESSES: u32 = 1_000_000;
        for n in 0..PRESSES {
            felt.pressed(What::Key, 7, || String::from("olo.styled-edit"), Some(ms(1)), t0);
            felt.drew(&7, t0 + ms(2));
            felt.framed("DP-2");
            std::hint::black_box(felt.shown("DP-2", t0 + ms(3 + u64::from(n % 2))));
        }
        println!("a press followed to the display: {:.0} ns", started.elapsed().as_secs_f64() * 1e9 / f64::from(PRESSES));
    }

    /// Kept: the last [`KEPT`], the oldest gone first.
    #[test]
    fn the_last_are_kept() {
        let t0 = Instant::now();
        let mut felt: Felt<u32> = Felt::default();
        for n in 0..(KEPT as u64 + 10) {
            let at = t0 + ms(n * 10);
            felt.pressed(What::Key, 1, || format!("app-{n}"), None, at);
            felt.drew(&1, at + ms(1));
            felt.framed("x");
            felt.shown("x", at + ms(2));
        }
        assert_eq!(felt.kept().count(), KEPT);
        assert_eq!(felt.kept().next().unwrap().app, "app-10");
    }
}
