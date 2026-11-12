//! The Olo Book's first program (docs/OLO-BOOK.md, "A whole program"):
//! counts the primes below a number on a worker, showing how far it has
//! got, and stops when told. Everything a kit app is made of, small: a
//! view with its state and its messages, a `body` of controls, a menu
//! bar and its commands, a child view (the text field) heard through
//! what it emits, work off the looper with progress and a way to stop
//! it, a drawing of the view's own, and what it keeps across an upgrade.
//! It's built with the kit, so the Book's code can't drift from it.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use olo_kit::{
    Child, Cx, Element, Menu, MenuBar, ParentElement as _, Styled as _, View, WindowOptions, div, metrics as m, px, style, theme,
    widgets::{Button, StatusBar, StringView, TextControl, TextField, TextFieldEvent, text_field},
};

/// The window's view: its state is everything it shows.
struct Primes {
    below: Child<TextField>,
    /// The count under way: which it is (each has a number, so what an
    /// earlier one's worker still says is known for old news), how far
    /// it is (0 to 1), and the flag that stops it.
    counting: Option<(u32, f32, Arc<AtomicBool>)>,
    counts: u32,
    /// How many were found in each tenth of the range, last time.
    found: Option<[u32; 10]>,
    said: String,
}

/// Everything that can happen to it.
#[derive(Clone)]
enum Msg {
    Count,
    Stop,
    /// From count `.0`'s worker: how far it is.
    Progress(u32, f32),
    /// From count `.0`'s worker: done (None: stopped).
    Counted(u32, Option<[u32; 10]>),
}

impl View for Primes {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Count => {
                let Ok(below) = self.below.read(cx).text().trim().parse::<u32>() else {
                    self.said = "A whole number, please.".into();
                    cx.invalidate();
                    return;
                };
                self.stop();
                let stop = Arc::new(AtomicBool::new(false));
                self.counts += 1;
                let which = self.counts;
                self.counting = Some((which, 0.0, stop.clone()));
                // The work is a worker's: the looper goes on taking
                // messages (and the window drawing) while it runs. It
                // says how far it is with messages of the view's own.
                let progress = cx.messenger();
                cx.work(
                    move || {
                        count(below, &stop, |done| {
                            progress.send(Msg::Progress(which, done));
                        })
                    },
                    move |found| Msg::Counted(which, found),
                );
            }
            Msg::Stop => self.stop(),
            Msg::Progress(which, done) => match &mut self.counting {
                Some((now, at, _)) if *now == which => *at = done,
                // (An earlier count's: its worker hadn't heard yet.)
                _ => return,
            },
            Msg::Counted(which, found) => {
                if self.counting.as_ref().map(|c| c.0) != Some(which) {
                    return;
                }
                self.counting = None;
                if let Some(found) = found {
                    self.said = format!("{} primes.", found.iter().sum::<u32>());
                    self.found = Some(found);
                }
            }
        }
        cx.invalidate();
    }

    /// What the menu bar's items, the command palette and agents choose.
    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        match command {
            "count" => cx.send(Msg::Count),
            "stop" => cx.send(Msg::Stop),
            "quit" => olo_kit::quit(),
            _ => return false,
        }
        true
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let counting = self.counting.as_ref().map(|(_, at, _)| *at);
        let menus = vec![Menu::new("File").item("Count", "count").key("k").item("Stop", "stop").enabled(counting.is_some()).separator().item("Quit", "quit").key("q")];
        let button = match counting {
            Some(_) => Button::new("Stop").on_click(cx.msg(Msg::Stop)),
            None => Button::new("Count").default(true).on_click(cx.msg(Msg::Count)),
        };
        // A drawing of the view's own: handed the view as it is when it's
        // drawn, and drawn again only when its key changes.
        let chart = cx
            .canvas(|v: &Primes, g, b| {
                let p = theme::palette();
                style::sunken(g, b, &p, false);
                let Some(found) = v.found else { return };
                let most = found.iter().copied().max().unwrap_or(1).max(1) as f32;
                // (Inside the frame's two lines, flush against them: a third
                // line in left a gap of one device pixel at 1.5×, which the
                // layout review found.)
                let inner = style::inset(g, b, 2.0);
                let wide = inner.size.width / 10.0;
                for (i, n) in found.iter().enumerate() {
                    let tall = inner.size.height * (*n as f32 / most);
                    let bar = olo_kit::Bounds::new(olo_kit::point(inner.left() + wide * i as f32, inner.bottom() - tall), olo_kit::size(wide - px(2.0), tall));
                    // (`style`'s fills land on whole device pixels at every scale.)
                    style::fill_rect(g, bar, style::c(p.control_highlight));
                }
            })
            .key_by(|v: &Primes| style::key(&v.found))
            .h(px(60.0));
        let content = div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .p(px(m::WINDOW_INSET))
            .child(div().flex().gap(px(m::SPACING)).child(div().flex_1().child(TextControl::new(cx, &self.below).label("Primes below:"))).child(button))
            .child(StatusBar::new(counting.unwrap_or(0.0)).label(if counting.is_some() { "Counting…" } else { "" }))
            .child(chart)
            .child(StringView::new(self.said.clone()).dim(true));
        div().size_full().flex().flex_col().child(MenuBar::new(cx, menus)).child(content)
    }

    /// Kept across an upgrade in place: handed back by
    /// `olo_kit::restored_state()` as the program starts again.
    fn save_state(&mut self, cx: &mut Cx<Self>) -> Option<String> {
        Some(self.below.read(cx).text().to_owned())
    }
}

impl Primes {
    fn stop(&mut self) {
        if let Some((_, _, stop)) = self.counting.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

/// The primes below `below`, a tenth of the range at a time; None once
/// `stop` is set. `progress` hears a hundred times at most: what would
/// flood a looper's queue is the sender's to gather.
fn count(below: u32, stop: &AtomicBool, progress: impl Fn(f32)) -> Option<[u32; 10]> {
    let mut found = [0; 10];
    let step = (below / 100).max(1);
    for n in 2..below {
        if n % step == 0 {
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            progress(n as f32 / below as f32);
        }
        if (2..).take_while(|d| d * d <= n).all(|d| n % d != 0) {
            found[(n as u64 * 10 / below as u64) as usize] += 1;
        }
    }
    Some(found)
}

fn main() {
    olo_kit::run("olo.book-example", |_| {
        olo_kit::open_window(WindowOptions::new("Primes", 360.0, 232.0), |cx| {
            let below = text_field(cx, |f| f.name("Primes below").with_text(olo_kit::restored_state().unwrap_or_else(|| "10000000".into())));
            // What a child view emits, its owner hears.
            cx.subscribe(&below, |_: &mut Primes, event: &TextFieldEvent, cx| {
                if matches!(event, TextFieldEvent::Entered) {
                    cx.send(Msg::Count);
                }
            });
            Primes { below, counting: None, counts: 0, found: None, said: "Nothing counted yet.".into() }
        });
    });
}
