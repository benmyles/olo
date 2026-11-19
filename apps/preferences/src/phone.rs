//! Phone: an iPhone's or an Android phone's messages, contacts and
//! one-time codes on this computer, which Olo speaks to itself over
//! Bluetooth (`olo_system::phone`; nothing else is installed).
//!
//! **Connecting a phone** is here, a step at a time, each seen to happen
//! (`connect`): 1. Find it (the phones nearby, as Bluetooth sees them; a
//! phone already paired with this computer for something else is used as
//! it is); 2. Confirm the code (the same six digits on both, shown
//! large); 3. Allow it on the phone (its own switches for this computer,
//! drawn as the phone shows them, each ticked as the phone turns it on).
//! Pairing is the Bluetooth panel's own (its watcher and its agent,
//! `crate::bluetooth::link`): one pairing, wherever it's started.
//!
//! Connected, the panel says how the phone stands (the phone service's
//! state file, followed by inotify, read off the UI thread) and has Olo's
//! settings for it (`phone.json`, which the service follows): what is
//! told of. One-time codes are Security ▸ Passwords' ([`codes`]: which
//! of the phone and the mail accounts are looked in, and where codes go),
//! said from here. What is asked of the service is asked on a worker
//! thread: the window never waits.

use std::{
    sync::{
        Arc, OnceLock,
        mpsc::{Sender, channel},
    },
    time::Duration,
};

use olo_kit::{
    Bounds, FontWeight, ParentElement as _, Pixels, SharedString, Styled as _, WindowCx, canvas, div, icons, metrics as m, point, px,
    style::{self, Align},
    theme,
    widgets::{BBox, Button, CheckBox, EmptyState, MenuField, RadioButton, StatusBar, StringView},
};
use olo_protocol::phone::{self, AgentCodes, Kind, Link as PhoneLink, Notify, PhoneSettings, PhoneState, Request};
use olo_sync::Mutex;
use olo_system::bluetooth::{self as bt, Ask};

use crate::{Msg, bluetooth, form::Ui};

enum Job {
    Save(PhoneSettings),
    /// Try the phone again now.
    Reconnect,
}

#[derive(Default)]
struct Shared {
    state: PhoneState,
    settings: PhoneSettings,
    /// The mail accounts set up (Integrations: the Google and Fastmail
    /// ones for Mail), for [`codes`].
    mail: Vec<olo_mail::Account>,
    /// The state file was read: something to show.
    known: bool,
    error: Option<String>,
}

struct Link {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Job>,
}

/// The phone service's state, followed, and the worker that asks it
/// (one for the app).
fn link(proxy: olo_kit::Proxy) -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        let shared: Arc<Mutex<Shared>> = Default::default();
        // Tests (the layout lint): a phone connected, and no service.
        if cfg!(test) {
            let mut s = shared.lock().unwrap();
            (s.state, s.known) = (test_state(), true);
            s.mail = vec![
                olo_mail::Account { provider: olo_mail::Provider::Gmail, address: "ada@example.com".into() },
                olo_mail::Account { provider: olo_mail::Provider::Fastmail, address: "ada@example.org".into() },
            ];
            drop(s);
            return Link { shared, tx: channel().0 };
        }
        let read = {
            let (shared, proxy) = (shared.clone(), proxy.clone());
            move |_: &[std::path::PathBuf]| {
                let (state, settings, mail) = (PhoneState::load(), PhoneSettings::load(), olo_mail::accounts());
                let mut s = shared.lock().unwrap();
                if s.known && s.state == state && s.settings == settings && s.mail == mail {
                    return;
                }
                (s.state, s.settings, s.mail, s.known) = (state, settings, mail, true);
                drop(s);
                proxy.send(Msg::Redraw);
            }
        };
        let mut paths = vec![PhoneState::path(), PhoneSettings::path()];
        paths.extend(olo_mail::watched());
        let first = read.clone();
        std::thread::spawn(move || first(&[]));
        if let Some(w) = olo_kit::watch::watch(paths, read) {
            // The app's, for as long as it runs.
            std::mem::forget(w);
        }
        let (tx, rx) = channel::<Job>();
        let sh = shared.clone();
        std::thread::Builder::new()
            .name("phone".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    let error = match job {
                        Job::Save(s) => s.save().err().map(|e| format!("Couldn't save: {e}")),
                        Job::Reconnect => match phone::ask(&Request::Reconnect, Duration::from_secs(10)) {
                            Ok(r) if r.ok => None,
                            Ok(r) => r.error,
                            Err(_) => Some("Olo’s phone service isn’t running.".to_owned()),
                        },
                    };
                    let mut s = sh.lock().unwrap();
                    if s.error != error {
                        s.error = error;
                        drop(s);
                        proxy.send(Msg::Redraw);
                    }
                }
            })
            .expect("thread");
        Link { shared, tx }
    })
}

/// Test data: a phone connected, with messages unread.
pub fn test_state() -> PhoneState {
    PhoneState {
        pid: 0,
        name: "Mira’s iPhone".into(),
        kind: Kind::IPhone,
        path: "/org/bluez/hci0/dev_02_00_00_00_F0_01".into(),
        link: PhoneLink::Ready,
        contacts: true,
        battery: Some(81),
        conversations: 38,
        unread: 2,
        codes: 0,
        code_at: None,
        trouble: String::new(),
        profiles: true,
        changes: 0,
        since: 0,
        music: None,
        others: true,
        model: String::new(),
        call: None,
    }
}

pub struct State {
    link: &'static Link,
    /// Bluetooth's own watcher and pairing agent.
    bluetooth: &'static bluetooth::Link,
    /// Looking for phones has been started (once, as the steps show).
    searched: bool,
}

impl State {
    pub fn new() -> State {
        State { link: link(olo_kit::proxy()), bluetooth: bluetooth::link(olo_kit::proxy()), searched: false }
    }
}

fn spawn(cmd: &str, app: &str) {
    if let Err(e) = olo_xdg::spawn_detached_app(cmd, app) {
        tracing::warn!("preferences: {cmd}: {e}");
    }
}

/// Room between the parts of the page (a step's rows have `SPACING`
/// between them; the page's parts half as much again).
const ROOM: f32 = m::SPACING * 1.5;

/// Where the steps are: three, the one being done lit, those done ticked.
fn stepper(at: usize) -> olo_kit::Element {
    const STEPS: [&str; 3] = ["Find your phone", "Confirm the code", "Allow it on the phone"];
    let drawn = canvas(move |g, b: Bounds<Pixels>| {
        let p = theme::palette();
        let y = b.center().y;
        // Across the whole width, as the header and the box under it:
        // the first step at the left edge, the last ending at the right
        // one, the middle one between them, a line from each to the next.
        let font = |now: bool| style::ui_font(if now { FontWeight::BOLD } else { FontWeight::NORMAL });
        let widths: Vec<f32> = STEPS.iter().enumerate().map(|(i, name)| 30.0 + f32::from(style::text_width(SharedString::from(*name), style::PLAIN_SIZE, font(i == at)))).collect();
        let (left, right) = (f32::from(b.left()), f32::from(b.right()));
        let starts = [left, ((left + right - widths[1]) / 2.0).round(), right - widths[2]];
        for (i, name) in STEPS.iter().enumerate() {
            let x = starts[i];
            let (done, now) = (i < at, i == at);
            // A disc with the step's number (a tick once done), and its
            // name beside it.
            let disc = style::snap_centred(g, point(px(x + 11.0), y), 22.0, 22.0);
            let fill = if done {
                p.success
            } else if now {
                p.keyboard_navigation
            } else {
                olo_kit::palette::mix(p.panel_text, p.panel_background, 0.75)
            };
            g.paint_quad(olo_kit::quad(disc, px(11.0), style::c(fill), px(0.0), style::c(fill), olo_kit::BorderStyle::default()));
            let mark: SharedString = if done { "✓".into() } else { (i + 1).to_string().into() };
            style::label(g, disc.center().x, y, Align::Center, mark, style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.panel_background));
            let ink = if now { style::c(p.panel_text) } else { style::secondary_text(&p) };
            style::label(g, px(x + 30.0), y, Align::Left, *name, style::PLAIN_SIZE, font(now), ink);
            if i + 1 < STEPS.len() {
                let (from, to) = (px(x + widths[i] + 12.0), px(starts[i + 1] - 12.0));
                if to > from {
                    style::hline(g, from, to, y, style::shade(&p, p.panel_background, 1.3));
                }
            }
        }
    })
    .paint_key(at as u64)
    .w_full()
    .h(px(28.0));
    olo_kit::IntoElement::into_any_element(drawn)
}

/// The phone's own switches for this computer, drawn as the phone shows
/// them: each a row with its name and a switch, on once the phone has
/// turned it on.
fn switches(rows: Vec<(&'static str, bool)>) -> olo_kit::Element {
    let key = rows.iter().fold(0u64, |k, (_, on)| k << 1 | u64::from(*on));
    let n = rows.len() as f32;
    let drawn = canvas(move |g, b: Bounds<Pixels>| {
        let p = theme::palette();
        // A card, as a phone's settings list: rounded, a hair between rows.
        let card = style::snap(g, b);
        let ground = olo_kit::palette::mix(p.panel_text, p.panel_background, 0.94);
        g.paint_quad(olo_kit::quad(card, px(10.0), style::c(ground), px(1.0), style::shade(&p, p.panel_background, 1.25), olo_kit::BorderStyle::default()));
        let row_h = f32::from(card.size.height) / n;
        for (i, (name, on)) in rows.iter().enumerate() {
            let top = f32::from(card.top()) + row_h * i as f32;
            let y = px(top + row_h / 2.0);
            if i > 0 {
                style::hline(g, card.left() + px(14.0), card.right(), px(top), style::shade(&p, p.panel_background, 1.2));
            }
            style::label(g, card.left() + px(14.0), y, Align::Left, *name, style::PLAIN_SIZE + 1.0, style::ui_font(FontWeight::NORMAL), style::c(p.panel_text));
            // The switch: a pill, green with its knob at the right when on.
            let pill = style::snap_centred(g, point(card.right() - px(14.0 + 21.0), y), 42.0, 24.0);
            let track = if *on { p.success } else { olo_kit::palette::mix(p.panel_text, p.panel_background, 0.7) };
            g.paint_quad(olo_kit::quad(pill, px(12.0), style::c(track), px(0.0), style::c(track), olo_kit::BorderStyle::default()));
            let knob_x = if *on { pill.right() - px(12.0) } else { pill.left() + px(12.0) };
            let knob = style::snap_centred(g, point(knob_x, y), 20.0, 20.0);
            g.paint_quad(olo_kit::quad(knob, px(10.0), style::c(p.document_background), px(0.0), style::c(p.document_background), olo_kit::BorderStyle::default()));
        }
    })
    .paint_key(key)
    .w(px(340.0))
    .h(px(44.0 * n));
    olo_kit::IntoElement::into_any_element(drawn)
}

/// A code, large: six digits in two groups, as phones show them.
fn code(digits: String) -> olo_kit::Element {
    let shown = if digits.len() == 6 { format!("{} {}", &digits[..3], &digits[3..]) } else { digits };
    let key = shown.bytes().fold(0u64, |k, b| k.wrapping_mul(31).wrapping_add(u64::from(b)));
    let drawn = canvas(move |g, b: Bounds<Pixels>| {
        let p = theme::palette();
        style::label(g, b.center().x, b.center().y, Align::Center, shown.clone(), 44.0, style::ui_font(FontWeight::BOLD), style::c(p.panel_text));
    })
    .paint_key(key)
    .w_full()
    .h(px(72.0));
    olo_kit::IntoElement::into_any_element(drawn)
}

/// Connecting a phone, a step at a time.
fn connect(ui: &mut Ui, st: &mut State, state: &PhoneState, error: Option<String>) {
    let (link, blue) = (st.link, st.bluetooth);
    let (status, doing, question, bt_error) = {
        let sh = blue.shared.lock().unwrap();
        let q = sh.question.as_ref().map(|q| (q.device.clone(), q.path.clone(), q.ask.clone()));
        (sh.status.clone(), sh.doing.clone(), q, sh.error.clone())
    };
    let paired = state.paired();
    let at = if paired {
        2
    } else if question.is_some() {
        1
    } else {
        0
    };

    // What it is, with its picture.
    let picture = canvas(|g, b: Bounds<Pixels>| {
        let at = style::snap_centred(g, b.center(), 48.0, 48.0);
        icons::paint(g, at, "phone");
    })
    .w(px(48.0))
    .h(px(48.0))
    .flex_shrink_0();
    let words = div().flex_1().min_w_0().flex().flex_col().gap(px(m::STACK)).child(olo_kit::widgets::heading("Connect your phone")).child(
        StringView::new("Your messages, your contacts and the one-time codes texted to you, on this computer: over Bluetooth, with nothing between and nothing else to install.").dim(true).wrap(true),
    );
    ui.space(m::SPACING * 0.5);
    ui.add(div().flex().flex_row().items_center().gap(px(ROOM)).child(picture).child(words));
    ui.space(m::SPACING * 0.5);
    ui.add(stepper(at));
    ui.space(m::SPACING * 0.5);

    let items = ui.collect(|ui| match (at, &status) {
        // (No Bluetooth to pair with.)
        (0 | 1, None) => ui.add(StringView::new("Looking at Bluetooth…").dim(true)),
        (0 | 1, Some(Err(_))) => ui.note("Bluetooth isn’t running on this computer."),
        (0, Some(Ok(status))) => {
            let Some(adapter) = &status.adapter else { return ui.note("This computer has no Bluetooth.") };
            if !adapter.powered {
                let path = adapter.path.clone();
                ui.add(div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().flex_1().child(StringView::new("Bluetooth is off."))).child(
                    Button::new("Turn On Bluetooth").default(true).on_click(move |_, _| {
                        let _ = blue.tx.send(bluetooth::Request::Power(path.clone(), true));
                    }),
                ));
                return;
            }
            // Looked for as the steps show, once; again when asked.
            if !st.searched {
                st.searched = true;
                let _ = blue.tx.send(bluetooth::Request::Search(adapter.path.clone()));
            }
            // An iPhone goes by what the computer says it is as they
            // pair: a hands-free device has its contacts offered, a
            // laptop not (Olo Doctor's fix, which needs a password).
            if !adapter.hands_free() {
                ui.add(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(m::SPACING))
                        .child(div().flex_1().min_w_0().child(StringView::new("An iPhone first: Bluetooth doesn’t yet present this computer as an iPhone expects, and the iPhone would offer it its notifications but not its contacts. Olo Doctor fixes it (Present as Hands-Free); then pair.").wrap(true)))
                        .child(Button::new("Open Olo Doctor").on_click(|_, _| {
                            if let Err(e) = olo_xdg::spawn_detached("olo-doctor") {
                                tracing::warn!("olo-doctor: {e}");
                            }
                        })),
                );
                ui.space(m::SPACING * 0.5);
            }
            ui.add(StringView::new("On the phone, open Settings ▸ Bluetooth and keep it open there. It appears here in a moment.").wrap(true));
            ui.space(m::SPACING * 0.5);
            let phones: Vec<&bt::Device> = status.devices.iter().filter(|d| d.icon == "phone" && (d.named || d.paired)).collect();
            if phones.is_empty() {
                ui.add(StringView::new(if adapter.discovering { "Looking for phones nearby…" } else { "No phone found yet." }).dim(true));
            }
            for d in phones {
                let busy = doing.get(&d.path).copied();
                let path = d.path.clone();
                let button = match busy {
                    Some(what) => Button::new(what).enabled(false),
                    None if d.paired => Button::new("Connect")
                        .default(true)
                        .on_click(move |_, _| bluetooth::act(blue, path.clone(), "Connecting…", "The phone didn’t connect. Is it unlocked, with Bluetooth on?", bt::connect)),
                    None => Button::new("Pair")
                        .default(true)
                        .on_click(move |_, _| bluetooth::act(blue, path.clone(), "Pairing…", "Pairing didn’t finish. Try again, with the phone unlocked on its Bluetooth settings.", bt::pair_phone)),
                };
                // (The nearest picture to what it's named: a phone not
                // yet paired says no more of itself.)
                let its = olo_protocol::phone::picture(Kind::Android, "", &d.name);
                let icon = canvas(move |g, b: Bounds<Pixels>| {
                    let at = style::snap_centred(g, b.center(), 16.0, 16.0);
                    icons::paint(g, at, its);
                })
                .w(px(16.0))
                .h(px(m::CONTROL_HEIGHT))
                .flex_shrink_0();
                let note = if d.paired { "Paired with this computer; not yet sharing its messages" } else { "Nearby" };
                ui.add(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(m::SPACING))
                        .child(icon)
                        .child(div().flex_1().min_w_0().flex().flex_col().child(StringView::new(d.name.clone()).bold(true).cut(true)).child(StringView::new(note).dim(true)))
                        .child(button),
                );
            }
            ui.space(m::SPACING * 0.5);
            let path = adapter.path.clone();
            ui.add(div().flex().flex_row().child(Button::new("Look Again").enabled(!adapter.discovering).on_click(move |_, _| {
                let _ = blue.tx.send(bluetooth::Request::Search(path.clone()));
            })));
        }
        (1, Some(Ok(_))) => {
            let Some((device, path, ask)) = question.clone() else { return };
            let answer = move |yes: bool| {
                move |_: &olo_kit::ClickEvent, _: &mut WindowCx| {
                    if let Some(q) = blue.shared.lock().unwrap().question.take() {
                        q.answer(yes.then(String::new));
                    }
                    if !yes {
                        bluetooth::cancel_pairing(path.clone());
                    }
                    olo_kit::proxy().send(Msg::Redraw);
                }
            };
            match ask {
                Ask::Confirm(n) | Ask::ShowPasskey(n) => {
                    ui.add(div().w_full().flex().flex_row().justify_center().child(StringView::new(format!("Does “{device}” show this number?")).bold(true)));
                    ui.add(code(format!("{n:06}")));
                    ui.add(div().w_full().flex().flex_row().justify_center().child(StringView::new("If it does, confirm here, and on the phone too.").dim(true)));
                    ui.space(m::SPACING * 0.5);
                    ui.add(
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .justify_center()
                            .gap(px(m::SPACING))
                            .child(Button::new("It’s Different").on_click(answer.clone()(false)))
                            .child(Button::new("It Matches").default(true).on_click(answer(true))),
                    );
                }
                _ => {
                    // (A phone that asks another way: Bluetooth's own
                    // panel has every question.)
                    ui.add(StringView::new(format!("“{device}” is asking something else to pair.")).wrap(true));
                    ui.add(div().flex().flex_row().child(Button::new("Open Bluetooth…").default(true).on_click(|_, _| {
                        crate::prefs().open("bluetooth");
                    })));
                }
            }
        }
        _ => {
            // Paired: the phone's own switches, as it shows them.
            let ready = state.link == PhoneLink::Ready;
            let (first, rows) = match state.kind {
                Kind::IPhone => (
                    format!("On “{}”: Settings ▸ Bluetooth ▸ tap ⓘ beside this computer, and turn these on. They can take a minute or two to appear there; leaving that page and opening it again shows them sooner. (If this computer is listed twice, look in both.)", state.name),
                    vec![("Show Notifications", ready), ("Sync Contacts", state.contacts)],
                ),
                Kind::Android => (
                    format!("On “{}”: allow this computer to see your messages and contacts when it asks (or in Bluetooth, the gear beside this computer).", state.name),
                    vec![("Text messages", ready), ("Contacts", state.contacts)],
                ),
            };
            ui.add(StringView::new(first).wrap(true));
            ui.space(m::SPACING * 0.5);
            ui.add(div().w_full().flex().flex_row().justify_center().child(switches(rows)));
            ui.space(m::SPACING * 0.5);
            let waiting = match state.link {
                // (There, and still not spoken to: why, as Bluetooth
                // says it.)
                PhoneLink::Away if !state.trouble.is_empty() => state.trouble.clone(),
                PhoneLink::Away => format!("“{}” isn’t in reach. Is it nearby, unlocked, with Bluetooth on?", state.name),
                PhoneLink::Taken => state.trouble.clone(),
                PhoneLink::Ready => "Messages are coming. Contacts follow once Sync Contacts is on.".to_owned(),
                _ => "Waiting for the phone… this page follows it by itself.".to_owned(),
            };
            let mut row = div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().flex_1().min_w_0().child(StringView::new(waiting).dim(true).wrap(true)));
            if state.link == PhoneLink::Away {
                row = row.child(Button::new("Try Again").default(true).on_click(move |_, _| {
                    let _ = link.tx.send(Job::Reconnect);
                }));
            }
            row = row.child(Button::new("Bluetooth…").on_click(|_, _| {
                crate::prefs().open("bluetooth");
            }));
            ui.add(row);
        }
    });
    // The step, in its box, with room inside.
    let inside = div().flex().flex_col().gap(px(m::SPACING)).py(px(m::SPACING * 0.5)).children(items);
    ui.add(BBox::new(["Find Your Phone", "Confirm the Code", "Allow It on the Phone"][at]).child(inside));
    if let Some(e) = bt_error.or(error) {
        ui.label(e);
    }
    if !state.trouble.is_empty() && !matches!(state.link, PhoneLink::Taken | PhoneLink::Away) {
        ui.note(state.trouble.clone());
    }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let link = st.link;
    let (state, settings, known, error) = {
        let s = link.shared.lock().unwrap();
        (s.state.clone(), s.settings.clone(), s.known, s.error.clone())
    };
    if !known {
        return ui.add(EmptyState::new("Looking for the phone…").icon("phone"));
    }
    // Not connected for the first time yet: the steps, and nothing else.
    // (Once it has been, a phone out of reach is the panel as it is,
    // saying so.)
    let set_up = state.paired() && (state.link == PhoneLink::Ready || state.conversations > 0 || state.contacts);
    if !set_up {
        return connect(ui, st, &state, error);
    }

    // How it stands.
    let connected = state.link.connected();
    let status = ui.collect(|ui| {
        let what = match state.link {
            PhoneLink::Allow => format!("Waiting for “{}” to allow it (its switches for this computer, in its Bluetooth settings).", state.name),
            PhoneLink::Away => format!("“{}” is out of reach or its Bluetooth is off. It’s tried again by itself.", state.name),
            PhoneLink::Taken => state.trouble.clone(),
            other => format!("{}: {}", state.name, other.words()),
        };
        ui.add(StringView::new(what).wrap(true));
        if let Some(b) = state.battery.filter(|_| connected) {
            ui.add(StatusBar::new(f32::from(b) / 100.0).label("Battery").trailing(format!("{b}%")));
        }
        let unread = match state.unread {
            0 => String::new(),
            n => format!(", {n} unread"),
        };
        ui.add(StringView::new(format!("{} conversations{unread}{}", state.conversations, if state.contacts { "" } else { " · its contacts haven’t come (Sync Contacts, on the phone)" })).dim(true));
        let mut buttons = div().flex().flex_row().flex_wrap().gap(px(m::SPACING));
        buttons = buttons.child(Button::new("Open Phone").on_click(move |_, _| spawn("olo-phone", "olo.phone")));
        if !connected {
            buttons = buttons.child(Button::new("Try Again").on_click(move |_, _| {
                let _ = link.tx.send(Job::Reconnect);
            }));
        }
        buttons = buttons.child(Button::new("Bluetooth…").on_click(|_, _| {
            crate::prefs().open("bluetooth");
        }));
        ui.add(buttons);
    });
    // (Its own picture beside what's said of it: the nearest of the
    // phones drawn to what it says it is.)
    let its = state.picture();
    let picture = canvas(move |g, b: Bounds<Pixels>| {
        let at = style::snap_centred(g, b.center(), 48.0, 48.0);
        icons::paint(g, at, its);
    })
    .w(px(48.0))
    .h(px(48.0))
    .flex_shrink_0();
    let said = div().flex_1().min_w_0().flex().flex_col().gap(px(m::SPACING)).children(status);
    let inside = div().flex().flex_row().items_start().gap(px(m::SPACING)).py(px(m::SPACING * 0.5)).child(picture).child(said);
    ui.add(BBox::new(if state.kind == Kind::IPhone { "iPhone" } else { "Phone" }).child(inside));
    ui.space(m::SPACING * 0.5);

    let save = move |change: fn(&mut PhoneSettings, bool)| {
        move |on: &bool, _: &mut WindowCx| {
            let mut s = link.shared.lock().unwrap().settings.clone();
            change(&mut s, *on);
            link.shared.lock().unwrap().settings = s.clone();
            let _ = link.tx.send(Job::Save(s));
        }
    };

    // (Two columns: what's told of; where codes go.)
    let left = ui.collect(|ui| {
        let items = ui.collect(|ui| {
            ui.label("Tell me of:");
            for choice in Notify::ALL {
                let detail = match choice {
                    Notify::Messages => "Messages: each as it comes, answered in place",
                    Notify::Nothing => "Nothing",
                };
                ui.add(RadioButton::new(detail, settings.notify == choice).on_select(move |_, _| {
                    let mut s = link.shared.lock().unwrap().settings.clone();
                    s.notify = choice;
                    link.shared.lock().unwrap().settings = s.clone();
                    let _ = link.tx.send(Job::Save(s));
                }));
            }
            ui.add(CheckBox::new("Show what a message says", settings.show_text).enabled(settings.notify == Notify::Messages).on_toggle(save(|s, on| s.show_text = on)));
            // (An iPhone's, of every app, calls among them: its Show
            // Notifications switch for this computer allows them.)
            ui.add(CheckBox::new("Show the phone’s other notifications", settings.others).on_toggle(save(|s, on| s.others = on)));
            // (Answered or made here, a call's sound is here: the
            // hands-free profile, for that call alone.)
            ui.add(CheckBox::new("Take calls on this computer", settings.calls).on_toggle(save(|s, on| s.calls = on)));
        });
        ui.add(BBox::new("Notifications").child(div().flex().flex_col().gap(px(m::SPACING)).py(px(m::SPACING * 0.5)).children(items)));
    });
    // (One-time codes are set in Security ▸ Passwords, with the mail
    // accounts': said here, a button away.)
    let right = ui.collect(|ui| {
        let items = ui.collect(|ui| {
            let said = if settings.codes { "Codes texted to this phone are offered in the browser tab waiting for them." } else { "Codes texted to this phone aren’t looked for." };
            ui.add(StringView::new(format!("{said} What’s looked in for codes, and where they go: Security ▸ Passwords.")).wrap(true));
            ui.add(div().flex().flex_row().child(Button::new("Security…").on_click(|_, _| crate::prefs().handle_args(&["passwords".to_owned()]))));
        });
        ui.add(BBox::new("One-Time Codes").child(div().flex().flex_col().gap(px(m::SPACING)).py(px(m::SPACING * 0.5)).children(items)));
    });
    ui.add_columns(left, right);
    if let Some(e) = error {
        ui.label(e);
    }
}

/// One-time codes (Security ▸ Passwords; Ben, 2026-11-17: "move the
/// phone prefs one-time codes stuff into prefs -> security -> passwords
/// and change it up a bit so that you can choose what to monitor"): which
/// of the phone set up and the mail accounts (Integrations' Google and
/// Fastmail ones for Mail) are
/// looked in, and where a code goes. Saved in `phone.json`, which the
/// phone service, the one that gives codes to the browsers, follows.
pub fn codes(ui: &mut Ui) {
    let link = link(olo_kit::proxy());
    let (state, settings, mail) = {
        let s = link.shared.lock().unwrap();
        (s.state.clone(), s.settings.clone(), s.mail.clone())
    };
    let save = move |s: PhoneSettings| {
        link.shared.lock().unwrap().settings = s.clone();
        let _ = link.tx.send(Job::Save(s));
    };
    let toggle = move |change: fn(&mut PhoneSettings, bool)| {
        move |on: &bool, _: &mut WindowCx| {
            let mut s = link.shared.lock().unwrap().settings.clone();
            change(&mut s, *on);
            save(s);
        }
    };
    let phone = state.paired();
    let watched = |a: &olo_mail::Account| !settings.unwatched_mail.contains(&a.address);
    let any = (phone && settings.codes) || mail.iter().any(watched);
    let items = ui.collect(|ui| {
        ui.label("Look for codes in:");
        if phone {
            ui.add(CheckBox::new(format!("Text messages to “{}”", state.name), settings.codes).on_toggle(toggle(|s, on| s.codes = on)));
        }
        for account in &mail {
            let address = account.address.clone();
            ui.add(CheckBox::new(format!("Mail to {} ({})", account.address, account.provider.name()), watched(account)).on_toggle(move |on: &bool, _: &mut WindowCx| {
                let mut s = link.shared.lock().unwrap().settings.clone();
                s.unwatched_mail.retain(|a| *a != address);
                if !*on {
                    s.unwatched_mail.push(address.clone());
                }
                save(s);
            }));
        }
        match (phone, mail.is_empty()) {
            (false, true) => ui.note("No phone or mail account is set up: a phone in Phone, a Google or Fastmail account for Mail in Integrations."),
            (true, true) => ui.note("Google and Fastmail accounts for Mail (Integrations) are looked in too."),
            (false, false) => ui.note("Mail is read only while a page waits for a code, for up to a minute. A phone, set up in Phone, is looked in too."),
            (true, false) => ui.note("Mail is read only while a page waits for a code, for up to a minute."),
        }
        ui.enabled_if(any, |ui| {
            ui.add(CheckBox::new("Offer a code in the browser tab waiting for it", settings.codes_in_browser).enabled(ui.enabled()).on_toggle(toggle(|s, on| s.codes_in_browser = on)));
            let names: Vec<SharedString> = AgentCodes::ALL.iter().map(|a| SharedString::from(a.label())).collect();
            let at = AgentCodes::ALL.iter().position(|a| *a == settings.agent_screens).unwrap_or(0);
            ui.add(MenuField::new("phone-agent-codes", names, at).label("In an agent’s tab:").enabled(ui.enabled() && settings.codes_in_browser).on_change(move |i, _| {
                let mut s = link.shared.lock().unwrap().settings.clone();
                s.agent_screens = AgentCodes::ALL[*i];
                save(s);
            }));
            ui.note(match settings.agent_screens {
                AgentCodes::Give => "A code that is surely an agent’s tab’s is put in there, and you’re told; one that isn’t sure is asked about.",
                AgentCodes::Ask => "You’re asked each time a tab on an Agent Screen is the one waiting.",
                AgentCodes::Never => "Tabs on Agent Screens are never given a code.",
            });
            ui.add(CheckBox::new("Copy a code no tab is waiting for", settings.copy_codes).enabled(ui.enabled()).on_toggle(toggle(|s, on| s.copy_codes = on)));
        });
    });
    ui.add(BBox::new("One-Time Codes").child(div().flex().flex_col().gap(px(m::SPACING)).py(px(m::SPACING * 0.5)).children(items)));
}
