//! Try Voice Processing (Preferences ▸ Sound ▸ Voice Processing ▸ Try
//! It…): a few seconds recorded from the microphone voice processing
//! cleans up, put through each setting worth comparing, as the live
//! service would (`olo_voice::trial`), and listed with what each adds:
//! its delay, its time a 10 ms frame (median and worst), what each stage
//! did to the sound, and what ran it (the processor's vector
//! instructions). Ben (2026-11-02, 2026-11-03): record a short sample,
//! show "in real time some kind of audio frequency graph/visualization
//! and what it's doing", report the delay, play it back "both with and
//! without the processing applied"; and "hearing how it sounds on the
//! different settings". Playing switches between the original and the
//! chosen setting's sound at the same place ([`olo_system::recording`]),
//! with a live spectrum of both (the kit's `Spectrum`); the one chosen
//! can be made the setting at once.
//!
//! Recording, trying and playing run off the UI thread; the recording
//! stays in memory and goes with the window. Echo cancellation isn't
//! tried: a recording has no speakers' sound beside it to take out.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use olo_kit::{
    Child, Cx, Element, IntoElement, Messenger, ParentElement as _, Styled as _, View, WindowCx, canvas, div, metrics as m, px, style, theme,
    widgets::{Button, Column, ColumnListView, LegendItem, ListModel, RadioButton, Spectrum, SpectrumSeries, StatusBar, StringView, legend, legend_height},
};
use olo_protocol::voice::{self, Settings};
use olo_sync::Mutex;
use olo_system::recording::{Player, Recording};

/// How long a recording runs, at most (seconds).
pub const SECONDS: f32 = 6.0;
/// The spectrum's bands.
const BANDS: usize = 48;

/// A setting tried: its name, the settings, and how it went once done.
pub struct Tried {
    pub name: String,
    pub settings: Settings,
    pub trial: Option<Result<olo_voice::Trial, String>>,
}

/// The settings worth comparing, from those in use: those, then each
/// other noise level, the level evened out or not, and the other mode.
pub fn variants(now: Settings) -> Vec<(String, Settings)> {
    let now = Settings { enabled: true, ..now };
    let mut out = vec![("As set now".to_owned(), now)];
    for n in voice::Noise::ALL.into_iter().filter(|n| *n != now.noise) {
        out.push((format!("Background noise: {}", n.label()), Settings { noise: n, ..now }));
    }
    if now.noise != voice::Noise::Off {
        for model in voice::Model::ALL.into_iter().filter(|m| *m != now.model) {
            out.push((format!("Noise model: {}", model.label()), Settings { model, ..now }));
        }
    }
    out.push((if now.normalize { "Level not evened out".into() } else { "Level evened out".into() }, Settings { normalize: !now.normalize, ..now }));
    for mode in voice::Mode::ALL.into_iter().filter(|m| *m != now.mode) {
        out.push((format!("Tuned for {}", mode.label().to_lowercase()), Settings { mode, ..now }));
    }
    out
}

/// Where the window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing recorded yet.
    Ready,
    Recording,
    /// Trying each setting on the recording.
    Trying,
    /// Every setting tried.
    Tried,
}

/// What the workers share with the window.
#[derive(Default)]
struct Shared {
    recording: Option<Recording>,
    original: Option<Arc<[f32]>>,
    tried: Vec<Tried>,
    error: Option<String>,
    trying: bool,
}

pub struct TryWindow {
    settings: Settings,
    microphone: Option<String>,
    shared: Arc<Mutex<Shared>>,
    started: Option<Instant>,
    /// The window is closing: everything stops.
    stop: Arc<AtomicBool>,
    /// Recording is to stop now (what came is tried).
    enough: Arc<AtomicBool>,
    player: Arc<Mutex<Option<Player>>>,
    /// Hearing the chosen setting's sound (else the original).
    processed: bool,
    list: ListModel,
    /// What moves while recording and playing (the spectrum, the status
    /// line): a view of its own, saying what it shows each frame then,
    /// while the rest says it again only when it changes (every frame of
    /// all of it was 1.8 ms).
    live: Child<LivePane>,
    /// The workers' news: the window says what it shows again.
    news: Messenger<()>,
}

/// The spectrum and the status line. It reads what the workers share
/// with the window itself; what's the window's own (when recording
/// began, which sound is heard) it is told (`TryWindow::changed`).
pub struct LivePane {
    shared: Arc<Mutex<Shared>>,
    player: Arc<Mutex<Option<Player>>>,
    list: ListModel,
    started: Option<Instant>,
    processed: bool,
    /// The spectra shown, smoothed: the original's and the processed's.
    shown: (Vec<f32>, Vec<f32>),
    /// Playing when last drawn (its end changes the buttons).
    was_playing: bool,
}

/// Playing came to its end (the live pane saw it): the buttons change.
struct PlayEnded;

/// Where it is: recording, trying, tried, or nothing yet.
fn phase_of(s: &Shared, started: Option<Instant>) -> Phase {
    if s.recording.is_some() || (started.is_some() && s.original.is_none() && s.error.is_none()) {
        Phase::Recording
    } else if s.trying {
        Phase::Trying
    } else if s.original.is_some() {
        Phase::Tried
    } else {
        Phase::Ready
    }
}

impl Drop for TryWindow {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let (shared, player) = (self.shared.clone(), self.player.clone());
        // (Stopping is a round trip to the sound server: not on this
        // thread.)
        std::thread::spawn(move || {
            drop(player.lock().unwrap().take());
            drop(shared.lock().unwrap().recording.take());
        });
    }
}

/// Open it.
pub fn open(settings: Settings, microphone: Option<String>, _: &mut WindowCx) {
    let options = olo_kit::WindowOptions::new("Try Voice Processing", 760.0, 600.0);
    olo_kit::open_window(options, move |cx| TryWindow::new(settings, microphone, cx));
}

impl TryWindow {
    fn new(settings: Settings, microphone: Option<String>, cx: &mut Cx<Self>) -> TryWindow {
        let (shared, player): (Arc<Mutex<Shared>>, Arc<Mutex<Option<Player>>>) = Default::default();
        let list = ListModel::new(false);
        let live = {
            let (shared, player, list) = (shared.clone(), player.clone(), list.clone());
            cx.new_view(|_| LivePane { shared, player, list, started: None, processed: true, shown: (vec![-120.0; BANDS], vec![-120.0; BANDS]), was_playing: false })
        };
        cx.subscribe(&live, |w: &mut TryWindow, _: &PlayEnded, cx| w.changed(cx));
        TryWindow { settings, microphone, shared, started: None, stop: Default::default(), enough: Default::default(), player, processed: true, list, live, news: cx.messenger() }
    }

    /// Something shown changed: every part says what it shows again.
    fn changed(&mut self, cx: &mut Cx<Self>) {
        cx.notify();
        let (started, processed) = (self.started, self.processed);
        self.live.update(cx, |l, cx| {
            l.started = started;
            l.processed = processed;
            cx.notify();
        });
    }

    pub fn phase(&self) -> Phase {
        phase_of(&self.shared.lock().unwrap(), self.started)
    }

    /// Record, from the microphone voice processing listens to (not its
    /// own cleaned-up source); then try each setting on it.
    fn record(&mut self, cx: &mut Cx<Self>) {
        self.stop_playing();
        {
            let mut s = self.shared.lock().unwrap();
            *s = Shared::default();
        }
        self.started = Some(Instant::now());
        self.enough.store(false, Ordering::Relaxed);
        let (shared, stop, cut, news, mic, settings) = (self.shared.clone(), self.stop.clone(), self.enough.clone(), self.news.clone(), self.microphone.clone(), self.settings);
        std::thread::spawn(move || {
            match Recording::start(mic.as_deref()) {
                Ok(r) => shared.lock().unwrap().recording = Some(r),
                Err(e) => {
                    shared.lock().unwrap().error = Some(e);
                    news.send(());
                    return;
                }
            }
            news.send(());
            let enough = (olo_system::recording::RATE as f32 * SECONDS) as usize;
            while !stop.load(Ordering::Relaxed) && !cut.load(Ordering::Relaxed) && shared.lock().unwrap().recording.as_ref().is_some_and(|r| r.len() < enough) {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let Some(r) = shared.lock().unwrap().recording.take() else { return };
            let mut got = r.finish();
            got.truncate(enough);
            if got.len() < olo_system::recording::RATE as usize / 2 {
                shared.lock().unwrap().error = Some("Nothing came from the microphone.".into());
                news.send(());
                return;
            }
            let original: Arc<[f32]> = got.into();
            {
                let mut s = shared.lock().unwrap();
                s.original = Some(original.clone());
                s.tried = variants(settings).into_iter().map(|(name, settings)| Tried { name, settings, trial: None }).collect();
                s.trying = true;
            }
            news.send(());
            // Each setting in turn, each shown as it's done, after a short
            // one untimed: the first chain in a process sets up what every
            // later one shares (a 27 ms first frame, the rest 0.4).
            let _ = olo_voice::trial(settings, &original[..original.len().min(4800)]);
            let todo: Vec<Settings> = shared.lock().unwrap().tried.iter().map(|t| t.settings).collect();
            for (i, settings) in todo.into_iter().enumerate() {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let t = olo_voice::trial(settings, &original);
                if let Some(row) = shared.lock().unwrap().tried.get_mut(i) {
                    row.trial = Some(t);
                }
                news.send(());
            }
            shared.lock().unwrap().trying = false;
            news.send(());
        });
        self.changed(cx);
    }

    /// Stop recording now (it's tried as far as it went).
    fn stop_recording(&mut self, cx: &mut Cx<Self>) {
        self.enough.store(true, Ordering::Relaxed);
        self.changed(cx);
    }

    /// The takes playing can switch between: the original, then each
    /// setting's sound (none yet: the original's in its place).
    fn takes(&self) -> Vec<Arc<[f32]>> {
        let s = self.shared.lock().unwrap();
        let Some(original) = s.original.clone() else { return Vec::new() };
        let mut out = vec![original.clone()];
        out.extend(s.tried.iter().map(|t| match &t.trial {
            Some(Ok(t)) => Arc::from(t.out.as_slice()),
            _ => original.clone(),
        }));
        out
    }

    /// The setting chosen in the list.
    fn chosen(&self) -> usize {
        self.list.state().single().unwrap_or(0)
    }

    /// The take to hear: the original, or the chosen setting's.
    fn take_now(&self) -> usize {
        if self.processed { 1 + self.chosen() } else { 0 }
    }

    fn playing(&self) -> bool {
        self.player.lock().unwrap().as_ref().is_some_and(|p| !p.finished())
    }

    fn play(&mut self, cx: &mut Cx<Self>) {
        if self.playing() {
            self.stop_playing();
            self.changed(cx);
            return;
        }
        let takes = self.takes();
        if takes.is_empty() {
            return;
        }
        let (which, slot, shared, news) = (self.take_now(), self.player.clone(), self.shared.clone(), self.news.clone());
        std::thread::spawn(move || match Player::start(takes, which, 0) {
            Ok(p) => {
                *slot.lock().unwrap() = Some(p);
                news.send(());
            }
            Err(e) => {
                shared.lock().unwrap().error = Some(e);
                news.send(());
            }
        });
        self.changed(cx);
    }

    fn stop_playing(&mut self) {
        let slot = self.player.clone();
        std::thread::spawn(move || drop(slot.lock().unwrap().take()));
    }

    /// Hear the original or the processed, from where it is.
    fn hear(&mut self, processed: bool, cx: &mut Cx<Self>) {
        self.processed = processed;
        let which = self.take_now();
        if let Some(p) = self.player.lock().unwrap().as_ref() {
            p.choose(which);
        }
        self.changed(cx);
    }

    /// Make the chosen setting the one in use.
    fn use_chosen(&mut self, cx: &mut Cx<Self>) {
        let chosen = self.chosen();
        let Some(settings) = self.shared.lock().unwrap().tried.get(chosen).map(|t| t.settings) else { return };
        let settings = Settings { enabled: true, echo_cancellation: self.settings.echo_cancellation, ..settings };
        if let Err(e) = settings.save() {
            self.shared.lock().unwrap().error = Some(format!("couldn't save the setting: {e}"));
        }
        self.settings = settings;
        self.changed(cx);
    }
}

/// A duration in ms, to a tenth under 10.
fn ms(d: std::time::Duration) -> String {
    let v = d.as_secs_f64() * 1000.0;
    if v < 10.0 { format!("{v:.1} ms") } else { format!("{v:.0} ms") }
}

fn db(v: f64) -> String {
    if v.abs() < 0.05 { "—".into() } else { format!("{v:+.1} dB") }
}

impl LivePane {
    /// The spectra now: of what's coming in while recording, of the
    /// original and the processed where they're playing.
    fn spectra(&mut self) -> bool {
        let (original, processed) = {
            let s = self.shared.lock().unwrap();
            if let Some(r) = &s.recording {
                (Some(olo_voice::spectrum(&r.tail(olo_voice::SPECTRUM_WINDOW), BANDS)), None)
            } else if let Some(p) = self.player.lock().unwrap().as_ref().filter(|p| !p.finished()) {
                let at = p.position();
                let o = s.original.as_ref().map(|o| olo_voice::spectrum(&o[..at.min(o.len())], BANDS));
                let chosen = self.list.state().single().unwrap_or(0);
                let pr = s.tried.get(chosen).and_then(|t| t.trial.as_ref()).and_then(|t| t.as_ref().ok()).map(|t| olo_voice::spectrum(&t.out[..at.min(t.out.len())], BANDS));
                (o, pr)
            } else {
                (None, None)
            }
        };
        let live = original.is_some();
        // Smoothed, so a voice reads as a shape, not a flicker.
        let ease = |shown: &mut Vec<f32>, now: Option<Vec<f32>>| {
            let now = now.unwrap_or_else(|| vec![-120.0; BANDS]);
            for (s, n) in shown.iter_mut().zip(now) {
                *s = if n > *s { 0.5 * *s + 0.5 * n } else { 0.85 * *s + 0.15 * n };
            }
        };
        ease(&mut self.shown.0, original);
        ease(&mut self.shown.1, processed);
        live || self.shown.0.iter().chain(&self.shown.1).any(|v| *v > -100.0)
    }
}

impl View for TryWindow {
    /// The workers' news.
    type Message = ();

    fn message(&mut self, _: (), cx: &mut Cx<Self>) {
        self.changed(cx);
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let lead = "Record a few seconds as you talk on a call, with the room as it is; then hear and see what each setting does to it. The recording stays in memory and goes when you close this.";
        let controls = self.controls(cx);
        let col = div()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .size_full()
            .p(px(m::WINDOW_INSET))
            .child(StringView::new(lead).wrap(true))
            .child(self.live.element())
            .child(div().flex_1().min_h(px(0.0)).child(controls));
        div().size_full().bg(style::c(p.panel_background)).child(col)
    }
}

impl View for LivePane {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let live = self.spectra();
        // (Each frame while something moves: recording, playing, the
        // spectrum settling.)
        if live {
            let again = cx.invalidator();
            cx.on_next_frame(move |_| again.invalidate());
        }
        let playing = self.player.lock().unwrap().as_ref().is_some_and(|p| !p.finished());
        let (phase, error, recorded, rows, done) = {
            let s = self.shared.lock().unwrap();
            (phase_of(&s, self.started), s.error.clone(), s.recording.as_ref().map(|r| r.len()).unwrap_or(0), s.tried.len(), s.tried.iter().filter(|t| t.trial.is_some()).count())
        };
        let (processed, at) = (self.processed, self.player.lock().unwrap().as_ref().filter(|p| !p.finished()).map(|p| p.position()));
        // Its end changes the buttons (Stop to Play): the window hears.
        if self.was_playing && !playing {
            cx.emit(PlayEnded);
        }
        self.was_playing = playing;
        let p = theme::palette();
        let (grey, blue) = (style::c(olo_theme::mix(p.document_text, p.document_background, 0.45)), style::c(p.navigation_base));
        let series = vec![SpectrumSeries::new("Original", grey, self.shown.0.clone()).fill(true), SpectrumSeries::new("Processed", blue, self.shown.1.clone())];
        // Which line is which.
        let items = vec![LegendItem { color: grey, name: "Original".into(), value: "".into() }, LegendItem { color: blue, name: "Processed: the setting chosen below".into(), value: "".into() }];
        let tall = legend_height(728.0, &items);
        // What's going on, in a line.
        let rate = olo_system::recording::RATE as f32;
        let status: (String, Option<f32>) = match (phase, &error) {
            (_, Some(e)) => (e.clone(), None),
            (Phase::Ready, _) => ("Nothing recorded yet. Press Record and talk for a few seconds.".into(), None),
            (Phase::Recording, _) => (format!("Recording: {:.1} of {SECONDS:.0} seconds. Talk as you do on a call.", recorded as f32 / rate), Some(recorded as f32 / rate / SECONDS)),
            (Phase::Trying, _) => (format!("Trying each setting on it: {done} of {rows}…"), Some(done as f32 / rows.max(1) as f32)),
            (Phase::Tried, _) => match at {
                Some(at) => (format!("Playing the {} at {:.1} s.", if processed { "processed sound" } else { "original" }, at as f32 / rate), None),
                None => ("Choose a setting, then play the original or its sound: switching goes on from the same place.".into(), None),
            },
        };
        div()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .w_full()
            .child(canvas(move |g, b| legend(g, b, &items)).w_full().h(px(tall)))
            .child(div().w_full().h(px(170.0)).child(Spectrum::new(series, olo_voice::SPECTRUM_LOW, olo_voice::SPECTRUM_HIGH).floor(-100.0)))
            .child(match status.1 {
                Some(f) => StatusBar::new(f).label(status.0).into_element(),
                None => div().h(px(m::CONTROL_HEIGHT)).flex().items_center().child(StringView::new(status.0)),
            })
    }
}

impl TryWindow {
    /// The settings tried, the buttons and the note.
    fn controls(&mut self, cx: &mut Cx<Self>) -> Element {
        let phase = self.phase();
        let (error, rows, tried) = {
            let s = self.shared.lock().unwrap();
            let rows: Vec<(String, Option<Result<olo_voice::Trial, String>>)> = s.tried.iter().map(|t| (t.name.clone(), t.trial.clone())).collect();
            let tried: Vec<Settings> = s.tried.iter().map(|t| t.settings).collect();
            (s.error.clone(), rows, tried)
        };
        let (processed, list_model, settings, playing) = (self.processed, self.list.clone(), self.settings, self.playing());
        let mut col = div().flex().flex_col().gap(px(m::SPACING)).size_full();
        // Each setting tried.
        let columns = vec![
            Column::new("Setting", 240.0),
            Column::new("Delay", 72.0).right(),
            Column::new("10 ms frame", 96.0).right(),
            Column::new("Worst", 80.0).right(),
            Column::new("Noise", 92.0).right(),
            Column::new("Level", 92.0).right(),
        ];
        let in_use = settings;
        let same = move |s: &Settings| Settings { echo_cancellation: in_use.echo_cancellation, enabled: true, ..*s } == Settings { enabled: true, ..in_use };
        let (rows2, tried2) = (rows.clone(), tried.clone());
        let list = ColumnListView::new(cx, "voice-tried", &list_model, columns, rows.len(), move |_: &Self, cell, g| {
            let (name, trial) = &rows2[cell.row];
            let ok = trial.as_ref().and_then(|t| t.as_ref().ok());
            let using = tried2.get(cell.row).is_some_and(same);
            match cell.column {
                0 => cell.text(g, if using { format!("{name} (in use)") } else { name.clone() }),
                1 => cell.text(g, ok.map(|t| ms(t.latency)).unwrap_or_else(|| if trial.is_some() { "failed".into() } else { "…".into() })),
                2 => cell.text(g, ok.map(|t| ms(t.frame_median)).unwrap_or_default()),
                3 => cell.text(g, ok.map(|t| ms(t.frame_worst)).unwrap_or_default()),
                4 => cell.text(g, ok.map(|t| db(t.measured.noise.change_db())).unwrap_or_default()),
                5 => cell.text(g, ok.map(|t| db(t.measured.level.change_db())).unwrap_or_default()),
                _ => {}
            }
        })
        .empty_text("Each setting tried shows here.")
        .on_select(cx.listener(|w: &mut TryWindow, _: &(), cx| {
            let which = w.take_now();
            if let Some(p) = w.player.lock().unwrap().as_ref() {
                p.choose(which);
            }
            w.changed(cx);
        }));
        col = col.child(div().flex_1().min_h(px(120.0)).child(list));
        // The buttons.
        let recording = phase == Phase::Recording;
        let can_play = matches!(phase, Phase::Tried | Phase::Trying) && error.is_none();
        // (The first chosen until another is.)
        if !rows.is_empty() && list_model.state().single().is_none() {
            list_model.state_mut().select(0);
        }
        let chosen = list_model.state().single().unwrap_or(0);
        let chosen_in_use = tried.get(chosen).is_some_and(same);
        let chosen_done = rows.get(chosen).is_some_and(|r| matches!(r.1, Some(Ok(_)))) && !chosen_in_use;
        fn act<E: ?Sized + 'static>(cx: &Cx<TryWindow>, f: fn(&mut TryWindow, &mut Cx<TryWindow>)) -> impl Fn(&E, &mut WindowCx) + 'static {
            cx.listener(move |w: &mut TryWindow, _: &E, cx| f(w, cx))
        }
        let row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .child(if recording {
                Button::new("Stop Recording").on_click(act(cx, TryWindow::stop_recording))
            } else {
                Button::new(if phase == Phase::Ready { "Record" } else { "Record Again" }).enabled(phase != Phase::Trying).default(phase == Phase::Ready).on_click(act(cx, TryWindow::record))
            })
            .child(Button::new(if playing { "Stop" } else { "Play" }).enabled(can_play).on_click(act(cx, TryWindow::play)))
            // What playing plays (switched as it plays, at the same place),
            // as tall as the buttons, so their text shares a baseline.
            .child(RadioButton::new("Original", !processed).height(m::CONTROL_HEIGHT).enabled(can_play).on_select(act(cx, |w, cx| w.hear(false, cx))))
            .child(RadioButton::new("Processed", processed).height(m::CONTROL_HEIGHT).enabled(can_play).on_select(act(cx, |w, cx| w.hear(true, cx))))
            .child(div().flex_1())
            .child(Button::new("Use This Setting").enabled(chosen_done).on_click(act(cx, TryWindow::use_chosen)));
        col = col.child(row);
        col.child(StringView::new(format!("Run by {}. Echo cancellation isn't tried: a recording has no speakers' sound beside it to take out.", olo_voice::accelerated_by())).dim(true).wrap(true))
    }
}

/// Each state, for the layout lint (`ui_lint_try_voice`): a name and the
/// phase.
#[cfg(test)]
pub(crate) fn lint_states() -> Vec<(&'static str, Phase)> {
    vec![("nothing recorded", Phase::Ready), ("recording", Phase::Recording), ("trying", Phase::Trying), ("tried", Phase::Tried)]
}

#[cfg(test)]
impl TryWindow {
    /// The window in `phase`, with made-up figures (the longest each
    /// column shows) and a voice-like spectrum.
    pub(crate) fn for_lint(phase: Phase, cx: &mut Cx<Self>) -> TryWindow {
        let mut w = TryWindow::new(Settings::default(), None, cx);
        let voice: Vec<f32> = (0..BANDS).map(|b| -30.0 - (b as f32 - 10.0).abs() * 1.2).collect();
        let shown = (voice.clone(), voice.iter().map(|v| v - 8.0).collect());
        w.live.update(cx, |l, _| l.shown = shown);
        let trial = |settings: Settings| olo_voice::Trial {
            settings,
            out: Vec::new(),
            latency: std::time::Duration::from_millis(30),
            frame_median: std::time::Duration::from_micros(320),
            frame_worst: std::time::Duration::from_micros(12_400),
            measured: Default::default(),
        };
        let mut s = w.shared.lock().unwrap();
        match phase {
            Phase::Ready => {}
            Phase::Recording => {
                w.started = Some(Instant::now());
            }
            Phase::Trying | Phase::Tried => {
                s.original = Some(Arc::from(vec![0.0f32; 4800]));
                s.tried = variants(Settings::default())
                    .into_iter()
                    .enumerate()
                    .map(|(i, (name, settings))| Tried { name, settings, trial: (phase == Phase::Tried || i < 3).then(|| Ok(trial(settings))) })
                    .collect();
                s.trying = phase == Phase::Trying;
            }
        }
        drop(s);
        let started = w.started;
        w.live.update(cx, |l, _| l.started = started);
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_try_each_choice_once() {
        let now = Settings::default();
        let v = variants(now);
        assert_eq!(v[0].1, Settings { enabled: true, ..now });
        // Each other noise level, the other noise model, the level the
        // other way, the other mode.
        assert_eq!(v.len(), 1 + 3 + 1 + 1 + 1);
        assert!(v.iter().any(|(name, s)| s.model == voice::Model::LowDelay && name.contains("Low Delay")), "{v:?}");
        // With noise off, no model to try.
        let off = variants(Settings { noise: voice::Noise::Off, ..now });
        assert!(off.iter().all(|(_, s)| s.model == now.model), "{off:?}");
        let mut seen = std::collections::HashSet::new();
        assert!(v.iter().all(|(_, s)| seen.insert(format!("{s:?}"))), "{v:?}");
        assert!(v.iter().all(|(name, _)| !name.is_empty()));
    }
}
