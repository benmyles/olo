//! Security ▸ Face Unlock: a look at the infrared camera standing for the
//! account password (`olo_system::face`, olo-faced). Installing it puts
//! the service, its models and the PAM module in place as root
//! (`olo-faced install`, through pkexec: Olo's polkit agent asks); setting
//! up takes the password, then five looks; Forget and Remove undo them.
//! All of it on a worker: the window never waits on the service.

use olo_sync::Mutex;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        mpsc::{Sender, channel},
    },
};

use olo_kit::{
    Child, ClickEvent, Cx, IntoElement as _, SharedString, WindowCx,
    widgets::{Button, StatusBar, StringView, TextControl, TextField, TextFieldEvent},
};
use olo_system::face::Status;
use zeroize::Zeroizing;

use crate::{Msg, PanelView, form::Ui, integrations::buttons, panel};

enum Request {
    Look,
    Install,
    Uninstall,
    Enroll(Zeroizing<String>),
    Forget(Zeroizing<String>),
}

#[derive(Default)]
struct Shared {
    /// None until first asked.
    status: Option<Status>,
    /// Looks so far, while setting up.
    looks: Option<(u32, u32)>,
    busy: bool,
    error: Option<String>,
    note: Option<String>,
}

struct Link {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Request>,
}

/// Where olo-faced, the PAM module and the models are: beside this
/// program, or in the tree it was built in (`share/olo/build-dir`).
fn sources() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let exe = std::env::current_exe().ok()?;
    let exe = PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
    let here = exe.parent()?.to_path_buf();
    let built = std::fs::read_to_string(here.join("../share/olo/build-dir")).ok().map(|s| PathBuf::from(s.trim()));
    let dir = [Some(here), built].into_iter().flatten().find(|d| d.join("olo-faced").is_file() && d.join("libpam_olo_face.so").is_file())?;
    let models = olo_xdg::data_dirs().iter().map(|d| d.join("face")).chain(dir.ancestors().nth(2).map(|repo| repo.join("data/face"))).find(|m| m.join("sface.onnx").is_file())?;
    Some((dir.join("olo-faced"), models, dir.join("libpam_olo_face.so")))
}

/// `olo-faced ARGS` as root, through polkit (its first line of output
/// starting "error: " is what went wrong).
fn as_root(faced: &Path, args: &[&str]) -> Result<(), String> {
    if !olo_system::env::at_the_computer() {
        return Err("Face unlock is installed only in the person's own session (polkit asks there).".into());
    }
    let out = std::process::Command::new("pkexec").arg("--disable-internal-agent").arg(faced).args(args).output().map_err(|e| e.to_string())?;
    match out.status.code() {
        Some(0) => Ok(()),
        Some(126 | 127) => Err("An administrator's password is needed.".into()),
        _ => Err(String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.strip_prefix("error: ").map(str::to_owned)).unwrap_or_else(|| "It didn't work.".into())),
    }
}

fn link(proxy: olo_kit::Proxy) -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        let shared: Arc<Mutex<Shared>> = Default::default();
        let (tx, rx) = channel::<Request>();
        if cfg!(test) {
            // (The layout lint: installed, set up.)
            shared.lock().unwrap().status = Some(Status { installed: true, camera: true, enrolled: 5 });
            return Link { shared, tx };
        }
        let sh = shared.clone();
        let _ = tx.send(Request::Look);
        std::thread::Builder::new()
            .name("face-unlock".into())
            .spawn(move || {
                while let Ok(req) = rx.recv() {
                    let said = |s: &Arc<Mutex<Shared>>, error: Option<String>, note: Option<&str>| {
                        let mut s = s.lock().unwrap();
                        s.error = error;
                        s.note = note.map(str::to_owned);
                    };
                    if !matches!(req, Request::Look) {
                        sh.lock().unwrap().busy = true;
                        proxy.send(Msg::Redraw);
                    }
                    match req {
                        Request::Look => {}
                        Request::Install => match sources() {
                            None => said(&sh, Some("Face unlock's service isn't in this build (build the whole workspace).".into()), None),
                            Some((faced, models, pam)) => match as_root(&faced, &["install", "--models", &models.to_string_lossy(), "--pam", &pam.to_string_lossy()]) {
                                Ok(()) => said(&sh, None, Some("Installed. Set it up with your face next.")),
                                Err(e) => said(&sh, Some(format!("Face unlock wasn't installed: {e}")), None),
                            },
                        },
                        Request::Uninstall => match sources() {
                            None => said(&sh, Some("Face unlock's service isn't in this build.".into()), None),
                            Some((faced, ..)) => match as_root(&faced, &["uninstall"]) {
                                Ok(()) => said(&sh, None, Some("Face unlock was removed.")),
                                Err(e) => said(&sh, Some(format!("Face unlock wasn't removed: {e}")), None),
                            },
                        },
                        Request::Enroll(password) => {
                            let (s2, p2) = (sh.clone(), proxy.clone());
                            sh.lock().unwrap().looks = Some((0, 5));
                            let r = olo_system::face::enroll(&password, move |got, of| {
                                s2.lock().unwrap().looks = Some((got, of));
                                p2.send(Msg::Redraw);
                            });
                            sh.lock().unwrap().looks = None;
                            match r.as_deref() {
                                Ok("enrolled") => said(&sh, None, Some("Set up: a look now stands for your password.")),
                                Ok("wrong-password") => said(&sh, Some("That password isn't right.".into()), None),
                                Ok("no-face") => said(&sh, Some("Your face wasn't seen well enough: sit facing the camera, at arm's length, and try again.".into()), None),
                                Ok("no-camera") => said(&sh, Some("The infrared camera couldn't be opened (its shutter, or another program).".into()), None),
                                Ok(other) => said(&sh, Some(format!("It wasn't set up ({other}).")), None),
                                Err(e) => said(&sh, Some(format!("It wasn't set up ({e}).")), None),
                            }
                        }
                        Request::Forget(password) => match olo_system::face::forget(&password).as_deref() {
                            Ok("forgotten") => said(&sh, None, Some("Your face was forgotten.")),
                            Ok("wrong-password") => said(&sh, Some("That password isn't right.".into()), None),
                            Ok(other) => said(&sh, Some(format!("It wasn't forgotten ({other}).")), None),
                            Err(e) => said(&sh, Some(format!("It wasn't forgotten ({e}).")), None),
                        },
                    }
                    let status = olo_system::face::status();
                    let mut s = sh.lock().unwrap();
                    s.status = Some(status);
                    s.busy = false;
                    drop(s);
                    proxy.send(Msg::Redraw);
                }
            })
            .expect("thread");
        Link { shared, tx }
    })
}

/// What the password is wanted for.
#[derive(Clone, Copy, PartialEq)]
enum Asking {
    Enroll,
    Forget,
}

pub struct State {
    link: &'static Link,
    asking: Option<Asking>,
    password: Child<TextField>,
}

impl State {
    pub fn new(cx: &mut Cx<PanelView>) -> State {
        let password = crate::form::text_field(cx, |f| f.password(true));
        cx.subscribe(&password, |v, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Entered)
                && let Some(st) = state_of(v)
            {
                st.go(cx);
                cx.notify();
            }
        });
        State { link: link(olo_kit::proxy()), asking: None, password }
    }

    fn go(&mut self, cx: &mut Cx<PanelView>) {
        let typed = Zeroizing::new(self.password.read(cx).text().to_owned());
        let Some(asking) = self.asking else { return };
        if typed.is_empty() {
            return;
        }
        self.asking = None;
        self.password.update(cx, |f, cx| f.clear_secret(cx));
        let _ = self.link.tx.send(match asking {
            Asking::Enroll => Request::Enroll(typed),
            Asking::Forget => Request::Forget(typed),
        });
    }
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::Security(st) => Some(&mut st.keys.face),
        _ => None,
    }
}

fn act(ui: &Ui, f: impl Fn(&mut State, &mut Cx<PanelView>) + 'static) -> impl Fn(&ClickEvent, &mut WindowCx) + 'static {
    ui.on_click_view(move |v, cx| {
        if let Some(st) = state_of(v) {
            f(st, cx);
        }
        cx.notify();
    })
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let sh = st.link.shared.lock().unwrap();
    let (status, looks, busy, error, note) = (sh.status.clone(), sh.looks, sh.busy, sh.error.clone(), sh.note.clone());
    drop(sh);
    let link = st.link;
    ui.bbox("Face Unlock", |ui| {
        ui.note("A look at the infrared camera can stand for your password: sudo in Terminal, administrator requests, Passwords and the lock screen. Only infrared counts, so a photo on a screen can't pass it.");
        // (Ids of their own: the Security Keys box beside has a Set Up…
        // and a Remove too, and buttons with one id lose their clicks.)
        let ask = |ui: &mut Ui, what: Asking, label: &'static str| {
            Button::new(label)
                .id(format!("face-{label}"))
                .enabled(!busy)
                .on_click(act(ui, move |st, cx| {
                    st.asking = Some(what);
                    st.password.focus(cx);
                }))
                .into_element()
        };
        match &status {
            None => ui.note("Asking…"),
            Some(s) if !s.installed => {
                ui.add(StringView::new("Face unlock isn't installed.").wrap(true));
                ui.add(buttons(vec![Button::new("Install…").enabled(!busy).on_click(move |_, _| drop(link.tx.send(Request::Install))).into_element()]));
            }
            Some(s) if !s.camera => ui.add(StringView::new("There's no infrared camera.").wrap(true)),
            Some(s) if s.enrolled == 0 => {
                ui.add(StringView::new("Not set up.").wrap(true));
                if st.asking.is_none() {
                    let b = ask(ui, Asking::Enroll, "Set Up…");
                    ui.add(buttons(vec![b]));
                }
            }
            Some(s) => {
                ui.add(StringView::new(format!("Set up ({} looks).", s.enrolled)).wrap(true));
                if st.asking.is_none() {
                    let (again, forget) = (ask(ui, Asking::Enroll, "Set Up Again…"), ask(ui, Asking::Forget, "Forget…"));
                    ui.add(buttons(vec![again, forget, Button::new("Remove").id("face-remove").enabled(!busy).on_click(move |_, _| drop(link.tx.send(Request::Uninstall))).into_element()]));
                }
            }
        }
        if let Some(what) = st.asking {
            ui.add(
                StringView::new(match what {
                    Asking::Enroll => "Type your password, then look at the camera from arm's length, turning your head a little.",
                    Asking::Forget => "Type your password to forget your face.",
                })
                .wrap(true),
            );
            ui.add(TextControl::new(ui.cx, &st.password).label("Password:"));
            let typed = !st.password.read(ui.cx).text().is_empty();
            ui.add(buttons(vec![
                Button::new(if what == Asking::Enroll { "Set Up" } else { "Forget" }).default(true).enabled(typed).on_click(act(ui, |st, cx| st.go(cx))).into_element(),
                Button::new("Cancel")
                    .on_click(act(ui, |st, cx| {
                        st.asking = None;
                        st.password.update(cx, |f, cx| f.clear_secret(cx));
                    }))
                    .into_element(),
            ]));
        }
        if let Some((got, of)) = looks {
            ui.add(StatusBar::new(got as f32 / of.max(1) as f32).label(SharedString::from("Looking at your face…")).trailing(format!("{got} of {of}")));
        } else if busy {
            ui.note("Working…");
        } else if let Some(e) = error {
            ui.add(StringView::new(e).wrap(true));
        } else if let Some(n) = note {
            ui.note(n);
        }
    });
}
