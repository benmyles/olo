//! Touch ID: the fingers enrolled with fprintd, adding one (touch after
//! touch, with a progress bar) and removing one. fprintd calls run on
//! worker threads (`olo_system::fingerprint`); enrolling blocks on the
//! sensor, so it has its own thread and can be cancelled.

use std::{
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{RecvTimeoutError, Sender, channel},
    },
    time::Duration,
};

use olo_kit::{
    egui::{Context, Rect, Ui, UiBuilder, vec2},
    metrics as m,
    widgets::{BBox, Button, Column, ColumnListView, ListState, MenuField, StatusBar, StringView},
};
use olo_system::fingerprint::{self as fp, EnrollEvent, FINGERS, Stop};

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
    contexts: Arc<Mutex<Vec<Context>>>,
}

fn wake(contexts: &Mutex<Vec<Context>>) {
    contexts.lock().unwrap().iter().for_each(|c| c.request_repaint());
}

fn user() -> String {
    olo_system::auth::current_user().unwrap_or_default()
}

fn link() -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        let shared: Arc<Mutex<Shared>> = Default::default();
        if let Some(fake) = fake_fingers() {
            // From the first frame (the one the layout lint checks).
            shared.lock().unwrap().fingers = Some(Ok(fake));
        }
        let contexts: Arc<Mutex<Vec<Context>>> = Default::default();
        let (tx, rx) = channel::<Request>();
        let (sh, ctxs) = (shared.clone(), contexts.clone());
        std::thread::Builder::new()
            .name("touchid".into())
            .spawn(move || {
                let me = user();
                if let Some(fake) = fake_fingers() {
                    return fake_worker(&sh, &ctxs, &rx, fake);
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
                        wake(&ctxs);
                    }
                    let req = match rx.recv_timeout(Duration::from_secs(3)) {
                        Ok(r) => r,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    match req {
                        Request::Enroll(finger) => enroll(&sh, &ctxs, &me, finger),
                        Request::Delete(finger) => {
                            sh.lock().unwrap().busy = true;
                            wake(&ctxs);
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
                    wake(&ctxs);
                }
            })
            .expect("thread");
        Link { shared, tx, contexts }
    })
}

/// Enrol on this (the worker) thread; the window watches `enrolling`.
fn enroll(sh: &Arc<Mutex<Shared>>, ctxs: &Arc<Mutex<Vec<Context>>>, me: &str, finger: String) {
    let stop = Stop::default();
    {
        let mut s = sh.lock().unwrap();
        s.error = None;
        s.note = None;
        s.enrolling = Some(Enrolling { finger: finger.clone(), stages: 0, passed: 0, message: "Getting the sensor ready…".into(), stop: stop.clone() });
    }
    wake(ctxs);
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
        wake(ctxs);
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

fn fake_worker(sh: &Arc<Mutex<Shared>>, ctxs: &Arc<Mutex<Vec<Context>>>, rx: &std::sync::mpsc::Receiver<Request>, mut fingers: Vec<String>) {
    loop {
        sh.lock().unwrap().fingers = Some(Ok(fingers.clone()));
        wake(ctxs);
        let Ok(req) = rx.recv() else { return };
        match req {
            Request::Delete(f) => {
                fingers.retain(|x| *x != f);
                sh.lock().unwrap().note = Some(format!("{} was removed.", fp::finger_name(&f)));
            }
            Request::Enroll(f) => {
                let stop = Stop::default();
                sh.lock().unwrap().enrolling =
                    Some(Enrolling { finger: f.clone(), stages: 5, passed: 0, message: "Touch the sensor, lift, and touch again.".into(), stop: stop.clone() });
                for _ in 0..5 {
                    wake(ctxs);
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

#[derive(Default)]
pub struct State {
    list: ListState,
    /// The finger to add.
    add: Option<String>,
    /// Asking before removing this finger.
    confirm: Option<String>,
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let link = link();
    {
        let mut ctxs = link.contexts.lock().unwrap();
        if !ctxs.iter().any(|c| c == ui.ctx()) {
            ctxs.push(ui.ctx().clone());
        }
    }
    let sh = link.shared.lock().unwrap();
    let fingers = match &sh.fingers {
        None => {
            ui.add(StringView::new("Looking for the fingerprint sensor…").dim(true));
            return;
        }
        Some(Err(why)) => {
            ui.add(StringView::new(why).dim(true));
            return;
        }
        Some(Ok(f)) => f.clone(),
    };
    let (busy, error, note) = (sh.busy, sh.error.clone(), sh.note.clone());
    let enrolling = sh.enrolling.as_ref().map(|e| (e.finger.clone(), e.stages, e.passed, e.message.clone(), e.stop.clone()));
    drop(sh);

    ui.add(StringView::new("Touch ID unlocks the screen and approves administrator requests."));
    BBox::new(&format!("Fingerprints for {}", user())).show(ui, |ui| {
        let columns = [Column::new("Finger", 300.0)];
        let list = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), 104.0));
        ui.scope_builder(UiBuilder::new().max_rect(list), |ui| {
            ColumnListView::new("fingers", &columns, fingers.len(), &mut st.list).show(ui, |cell| cell.text(&fp::finger_name(&fingers[cell.row])));
        });
        ui.advance_cursor_after_rect(list);
        let selected = st.list.single().and_then(|i| fingers.get(i)).cloned();
        let free: Vec<&str> = FINGERS.iter().copied().filter(|f| !fingers.iter().any(|e| e == f)).collect();
        let names: Vec<String> = free.iter().map(|f| fp::finger_name(f)).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        // The usual choices first.
        const LIKELY: [&str; 4] = ["right-index-finger", "left-index-finger", "right-middle-finger", "right-thumb"];
        if st.add.as_ref().is_none_or(|a| !free.contains(&a.as_str())) {
            st.add = LIKELY.iter().find(|f| free.contains(f)).or(free.first()).map(|f| (*f).to_owned());
        }
        let mut add = st.add.as_ref().and_then(|a| free.iter().position(|f| f == a)).unwrap_or(0);
        let idle = enrolling.is_none() && !busy;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = m::SPACING;
            ui.add_enabled_ui(idle && !free.is_empty(), |ui| {
                ui.add(MenuField::new("add-finger", &mut add, &names).width(150.0));
                st.add = free.get(add).map(|f| (*f).to_owned());
                if ui.add(Button::new("Add Fingerprint")).clicked()
                    && let Some(f) = free.get(add)
                {
                    let _ = link.tx.send(Request::Enroll((*f).to_owned()));
                }
            });
            ui.add_enabled_ui(idle && selected.is_some(), |ui| {
                if ui.add(Button::new("Remove")).clicked() {
                    st.confirm = selected.clone();
                }
            });
        });
    });

    if let Some((finger, stages, passed, message, stop)) = enrolling {
        let fraction = if stages == 0 { 0.0 } else { passed as f32 / stages as f32 };
        ui.add(StatusBar::new(fraction).label(&format!("Enrolling your {}", fp::finger_name(&finger).to_lowercase())).trailing(&format!("{passed} of {stages}")));
        ui.horizontal(|ui| {
            ui.add(StringView::new(&message));
        });
        if ui.add(Button::new("Cancel")).clicked() {
            if fake_fingers().is_some() {
                stop.stop_flag();
            } else {
                std::thread::spawn(move || fp::stop_enroll(&stop));
            }
        }
    } else if let Some(finger) = st.confirm.clone() {
        ui.add(StringView::new(&format!("Remove the {}? It will no longer unlock anything.", fp::finger_name(&finger).to_lowercase())));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = m::SPACING;
            if ui.add(Button::new("Cancel")).clicked() {
                st.confirm = None;
            }
            if ui.add(Button::new("Remove").default(true)).clicked() {
                let _ = link.tx.send(Request::Delete(finger));
                st.confirm = None;
                st.list.selected.clear();
            }
        });
    } else if busy {
        ui.add(StringView::new("Working…").dim(true));
    } else if let Some(e) = error {
        ui.add(StringView::new(&e));
    } else if let Some(n) = note {
        ui.add(StringView::new(&n).dim(true));
    }
}
