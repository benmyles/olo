//! Touch ID, Security ▸ Sign In's box: the fingers enrolled with
//! fprintd, adding one (touch after touch, with a progress bar) and
//! removing one. fprintd calls run on
//! worker threads (`olo_system::fingerprint`); enrolling blocks on the
//! sensor, so it has its own thread and can be cancelled.

use olo_sync::Mutex;
use std::{
    sync::{
        Arc, OnceLock,
        mpsc::{RecvTimeoutError, Sender, channel},
    },
    time::Duration,
};

use olo_system::fingerprint::{self as fp, EnrollEvent, FINGERS, Stop};
use olo_ui::{
    gpui::{Context, Entity, ParentElement as _, SharedString, Styled as _, div, px},
    metrics as m,
    widgets::{Button, Column, ColumnListView, ListModel, MenuField, StatusBar},
};

use crate::{Msg, PanelView, form::Ui, panel};

enum Request {
    Enroll(String),
    Delete(String),
}

struct Enrolling {
    finger: String,
    stages: u32,
    passed: u32,
    message: String,
    stop: Stop,
}

#[derive(Default)]
struct Shared {
    /// None until first read; Err when fprintd can't be reached.
    fingers: Option<Result<Vec<String>, String>>,
    enrolling: Option<Enrolling>,
    busy: bool,
    error: Option<String>,
    note: Option<String>,
}

struct Link {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Request>,
}

fn user() -> String {
    olo_system::auth::current_user().unwrap_or_default()
}

/// fprintd's worker (one for the app), telling the windows through
/// `proxy` when anything changes.
fn link(proxy: olo_ui::Proxy) -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        let shared: Arc<Mutex<Shared>> = Default::default();
        if let Some(fake) = fake_fingers() {
            shared.lock().unwrap().fingers = Some(Ok(fake));
        }
        let (tx, rx) = channel::<Request>();
        let sh = shared.clone();
        std::thread::Builder::new()
            .name("touchid".into())
            .spawn(move || {
                let me = user();
                if let Some(fake) = fake_fingers() {
                    return fake_worker(&sh, &proxy, &rx, fake);
                }
                loop {
                    let fingers = Some(if fp::available() {
                        fp::fingers(&me).map_err(|e| {
                            if e.to_string().contains("Not Authorized") {
                                "Fingerprints can only be changed from a session logged in at this computer.".to_owned()
                            } else {
                                tracing::warn!("fprintd: {e}");
                                "The fingerprint sensor isn't answering.".to_owned()
                            }
                        })
                    } else {
                        Err("There's no fingerprint sensor, or fprintd isn't running.".into())
                    });
                    let changed = {
                        let mut s = sh.lock().unwrap();
                        std::mem::replace(&mut s.fingers, fingers.clone()) != fingers
                    };
                    if changed {
                        proxy.send(Msg::Redraw);
                    }
                    let req = match rx.recv_timeout(Duration::from_secs(3)) {
                        Ok(r) => r,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    match req {
                        Request::Enroll(finger) => enroll(&sh, &proxy, &me, finger),
                        Request::Delete(finger) => {
                            sh.lock().unwrap().busy = true;
                            proxy.send(Msg::Redraw);
                            let r = fp::delete(&me, &finger);
                            let mut s = sh.lock().unwrap();
                            s.busy = false;
                            s.error = r.err().map(|e| {
                                tracing::warn!("fprintd: {e}");
                                "The fingerprint couldn't be removed.".to_owned()
                            });
                            s.note = s.error.is_none().then(|| format!("{} was removed.", fp::finger_name(&finger)));
                        }
                    }
                    proxy.send(Msg::Redraw);
                }
            })
            .expect("thread");
        Link { shared, tx }
    })
}

/// Enrol on this (the worker) thread; the window watches `enrolling`.
fn enroll(sh: &Arc<Mutex<Shared>>, proxy: &olo_ui::Proxy, me: &str, finger: String) {
    let stop = Stop::default();
    {
        let mut s = sh.lock().unwrap();
        s.error = None;
        s.note = None;
        s.enrolling = Some(Enrolling { finger: finger.clone(), stages: 0, passed: 0, message: "Getting the sensor ready…".into(), stop: stop.clone() });
    }
    proxy.send(Msg::Redraw);
    let r = fp::enroll(me, &finger, &stop, |e| {
        let mut s = sh.lock().unwrap();
        if let Some(en) = s.enrolling.as_mut() {
            match e {
                EnrollEvent::Ready { stages } => {
                    en.stages = stages;
                    en.message = "Touch the sensor, lift, and touch again.".into();
                }
                EnrollEvent::Stage => {
                    en.passed += 1;
                    en.message = "Good. Lift, and touch again, a little differently.".into();
                }
                EnrollEvent::Retry(why) => en.message = why,
                EnrollEvent::Done => {}
                EnrollEvent::Failed(why) => s.error = Some(why),
            }
        }
        drop(s);
        proxy.send(Msg::Redraw);
    });
    let mut s = sh.lock().unwrap();
    let cancelled = s.enrolling.take().is_some_and(|e| e.stop.is_stopped());
    match r {
        Ok(true) => s.note = Some(format!("{} was added.", fp::finger_name(&finger))),
        Ok(false) if cancelled => {}
        Ok(false) => {
            s.error.get_or_insert_with(|| "Enrolling didn't finish.".to_owned());
        }
        Err(e) => {
            tracing::warn!("fprintd: {e}");
            s.error = Some("The fingerprint sensor couldn't be used.".into());
        }
    }
}

/// `OLO_FAKE_FINGERS=right-index-finger,left-thumb` (debug builds):
/// pretend to be a sensor with these fingers, so the panel can be tried
/// headless without touching the real sensor or the user's prints.
fn fake_fingers() -> Option<Vec<String>> {
    if cfg!(test) {
        // Tests always have some.
        return Some(["right-index-finger", "right-middle-finger", "left-thumb"].map(str::to_owned).to_vec());
    }
    if !cfg!(debug_assertions) {
        return None;
    }
    let v = std::env::var("OLO_FAKE_FINGERS").ok()?;
    Some(v.split(',').filter(|f| !f.is_empty()).map(str::to_owned).collect())
}

fn fake_worker(sh: &Arc<Mutex<Shared>>, proxy: &olo_ui::Proxy, rx: &std::sync::mpsc::Receiver<Request>, mut fingers: Vec<String>) {
    loop {
        sh.lock().unwrap().fingers = Some(Ok(fingers.clone()));
        proxy.send(Msg::Redraw);
        let Ok(req) = rx.recv() else { return };
        match req {
            Request::Delete(f) => {
                fingers.retain(|x| *x != f);
                sh.lock().unwrap().note = Some(format!("{} was removed.", fp::finger_name(&f)));
            }
            Request::Enroll(f) => {
                let stop = Stop::default();
                sh.lock().unwrap().enrolling = Some(Enrolling { finger: f.clone(), stages: 5, passed: 0, message: "Touch the sensor, lift, and touch again.".into(), stop: stop.clone() });
                for _ in 0..5 {
                    proxy.send(Msg::Redraw);
                    std::thread::sleep(Duration::from_millis(1500));
                    if stop.is_stopped() {
                        break;
                    }
                    if let Some(e) = sh.lock().unwrap().enrolling.as_mut() {
                        e.passed += 1;
                        e.message = "Good. Lift, and touch again, a little differently.".into();
                    }
                }
                let mut s = sh.lock().unwrap();
                s.enrolling = None;
                if !stop.is_stopped() {
                    fingers.push(f.clone());
                    s.note = Some(format!("{} was added.", fp::finger_name(&f)));
                }
            }
        }
    }
}

pub struct State {
    list: Entity<ListModel>,
    /// The finger to add.
    add: Option<String>,
    /// Asking before removing this finger.
    confirm: Option<String>,
    link: &'static Link,
}

impl State {
    pub fn new(cx: &mut Context<PanelView>) -> State {
        State { list: ListModel::new(cx, false), add: None, confirm: None, link: link(olo_ui::proxy(cx)) }
    }
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::Security(st) => Some(&mut st.touchid),
        _ => None,
    }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    ui.bbox("Touch ID", |ui| fingers(ui, st));
}

/// Touch ID's box: the fingers, and adding or removing one.
fn fingers(ui: &mut Ui, st: &mut State) {
    let link = st.link;
    let sh = link.shared.lock().unwrap();
    let fingers = match &sh.fingers {
        None => {
            drop(sh);
            ui.note("Looking for the fingerprint sensor…");
            return;
        }
        Some(Err(why)) => {
            let why = why.clone();
            drop(sh);
            ui.note(why);
            return;
        }
        Some(Ok(f)) => f.clone(),
    };
    let (busy, error, note) = (sh.busy, sh.error.clone(), sh.note.clone());
    let enrolling = sh.enrolling.as_ref().map(|e| (e.finger.clone(), e.stages, e.passed, e.message.clone(), e.stop.clone()));
    drop(sh);

    ui.note(format!("A finger on the fingerprint sensor can stand for your password. Fingers enrolled for {}:", user()));
    let selected = st.list.read(ui.cx).state.single().and_then(|i| fingers.get(i)).cloned();
    let free: Vec<&'static str> = FINGERS.iter().copied().filter(|f| !fingers.iter().any(|e| e == f)).collect();
    // The usual choices first.
    const LIKELY: [&str; 4] = ["right-index-finger", "left-index-finger", "right-middle-finger", "right-thumb"];
    if st.add.as_ref().is_none_or(|a| !free.contains(&a.as_str())) {
        st.add = LIKELY.iter().find(|f| free.contains(f)).or(free.first()).map(|f| (*f).to_owned());
    }
    let add = st.add.as_ref().and_then(|a| free.iter().position(|f| f == a)).unwrap_or(0);
    let idle = enrolling.is_none() && !busy;
    let names: Vec<SharedString> = free.iter().map(|f| fp::finger_name(f).into()).collect();
    let (pick_free, add_free) = (free.clone(), free.clone());
    let pick = ui.on_view(move |v, i: usize, _, cx| {
        if let Some(st) = state_of(v) {
            st.add = pick_free.get(i).map(|f| (*f).to_owned());
        }
        cx.notify();
    });
    let remove = {
        let selected = selected.clone();
        ui.on_click_view(move |v, _, cx| {
            if let Some(st) = state_of(v) {
                st.confirm = selected.clone();
            }
            cx.notify();
        })
    };
    let rows = fingers.clone();
    let list = ColumnListView::new("fingers", &st.list, vec![Column::new("Finger", 300.0)], fingers.len(), move |cell, window, cx| cell.text(window, cx, fp::finger_name(&rows[cell.row])))
        .on_select(|_, cx| cx.refresh_windows());
    ui.add(div().h(px(104.0)).child(list));
    ui.add(
        div()
            .flex()
            .flex_row()
            .gap(px(m::SPACING))
            .child(MenuField::new("add-finger", names, add).width(150.0).enabled(idle && !free.is_empty()).on_change(pick))
            .child(Button::new("Add Fingerprint").enabled(idle && !free.is_empty()).on_click(move |_, _| {
                if let Some(f) = add_free.get(add) {
                    let _ = link.tx.send(Request::Enroll((*f).to_owned()));
                }
            }))
            // (An id of its own: the Security Keys box beside has a Remove,
            // and buttons with one id lose their clicks.)
            .child(Button::new("Remove").id("finger-remove").enabled(idle && selected.is_some()).on_click(remove)),
    );

    if let Some((finger, stages, passed, message, stop)) = enrolling {
        let fraction = if stages == 0 { 0.0 } else { passed as f32 / stages as f32 };
        ui.add(StatusBar::new(fraction).label(format!("Enrolling your {}", fp::finger_name(&finger).to_lowercase())).trailing(format!("{passed} of {stages}")));
        ui.label(message);
        ui.add(Button::new("Cancel").id("finger-enroll-cancel").on_click(move |_, _| {
            let stop = stop.clone();
            if fake_fingers().is_some() {
                stop.stop_flag();
            } else {
                std::thread::spawn(move || fp::stop_enroll(&stop));
            }
        }));
    } else if let Some(finger) = st.confirm.clone() {
        ui.label(format!("Remove the {}? It will no longer unlock anything.", fp::finger_name(&finger).to_lowercase()));
        let cancel = ui.on_click_view(|v, _, cx| {
            if let Some(st) = state_of(v) {
                st.confirm = None;
            }
            cx.notify();
        });
        let list = st.list.clone();
        let confirm = ui.on_click_view(move |v, _, cx| {
            let _ = link.tx.send(Request::Delete(finger.clone()));
            if let Some(st) = state_of(v) {
                st.confirm = None;
            }
            list.update(cx, |l, _| l.state.selected.clear());
            cx.notify();
        });
        ui.add(
            div()
                .flex()
                .flex_row()
                .gap(px(m::SPACING))
                .child(Button::new("Cancel").id("finger-remove-cancel").on_click(cancel))
                .child(Button::new("Remove").id("finger-remove-confirm").default(true).on_click(confirm)),
        );
    } else if busy {
        ui.note("Working…");
    } else if let Some(e) = error {
        ui.label(e);
    } else if let Some(n) = note {
        ui.note(n);
    }
}
