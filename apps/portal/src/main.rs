//! olo-portal: Olo's xdg-desktop-portal backend (screen sharing first).
//! It serves `org.freedesktop.impl.portal.desktop.olo` on the session bus
//! (`portal.rs`), asks the person what to share in a kit dialog
//! (`picker.rs`), and shares screens as PipeWire streams (`stream.rs`)
//! of frames app_server draws into its buffers (`capture.rs`).
//!
//! ```text
//! olo-portal --service            the session's backend (started by olo-session)
//! olo-portal --bus ADDRESS        serve on another D-Bus (a test's own)
//! olo-portal capture-test OUT.png [OUTPUT]   write a frame of an output
//! olo-portal stream-test [OUTPUT|-] [SECONDS] share one as a PipeWire
//!                                 video source for that long
//! olo-portal share-test OUT.png [--bus ADDRESS] [--hold SECS] [--types 1|2|3]  ask
//!                                 xdg-desktop-portal for a screen, as an
//!                                 app does, write the first frame that
//!                                 comes, and keep sharing that long
//! olo-portal --stop SESSION       end a share (Deskbar's Stop Sharing)
//! ```

use std::collections::HashMap;

use olo_kit::{App, Cx, WindowId, egui, instance};

mod capture;
mod client;
mod picker;
mod portal;
mod stream;

const APP_ID: &str = "olo.portal";

/// A screen that can be shared, as app_server describes it.
pub struct ScreenInfo {
    pub output: String,
    pub label: String,
    /// In pixels, turned as the screen is.
    pub size: (u32, u32),
    /// Where it is and how big, in the desktop's logical coordinates.
    pub position: (i32, i32),
    pub logical: (i32, i32),
}

/// The screens (not mirrors: they show another's picture), left to right.
pub fn screens() -> Vec<ScreenInfo> {
    use olo_protocol::{Client, Reply, Request};
    let Ok(Reply::Outputs { mut outputs }) = Client::connect().and_then(|mut c| c.request(&Request::Outputs)) else { return Vec::new() };
    outputs.retain(|o| o.mirror_of.is_none());
    outputs.sort_by_key(|o| o.position);
    let names: Vec<String> = outputs.iter().map(|o| o.display_name()).collect();
    outputs
        .iter()
        .zip(&names)
        .map(|(o, name)| {
            let (w, h) = o.current_mode.as_ref().map(|m| (m.width.max(0) as u32, m.height.max(0) as u32)).unwrap_or_default();
            let size = if o.rotation % 180 == 90 { (h, w) } else { (w, h) };
            let scale = if o.scale > 0.0 { o.scale } else { 1.0 };
            // Two of the same monitor: told apart by their connectors.
            let label = if names.iter().filter(|n| *n == name).count() > 1 { format!("{name} ({})", o.name) } else { name.clone() };
            ScreenInfo { output: o.name.clone(), label, size, position: o.position, logical: ((size.0 as f64 / scale).round() as i32, (size.1 as f64 / scale).round() as i32) }
        })
        .collect()
}

/// Something that can be shared: a screen or a window.
pub struct Shareable {
    /// How a choice names it: a screen's connector, `window:ID`.
    pub key: String,
    pub label: String,
    pub window: bool,
    pub source: capture::Source,
    /// Its size, in pixels for a screen (logical for a window: only its
    /// shape counts, for its thumbnail).
    pub size: (u32, u32),
    /// Where it is and how big, in the desktop's logical coordinates.
    pub position: (i32, i32),
    pub logical: (i32, i32),
}

/// The screens, then the windows (not minimized, nor our own), as
/// app_server lists them.
pub fn shareables() -> Vec<Shareable> {
    use olo_protocol::{Client, Reply, Request};
    let mut all: Vec<Shareable> = screens()
        .into_iter()
        .map(|s| Shareable { key: s.output.clone(), label: s.label, window: false, source: capture::Source::Output(Some(s.output)), size: s.size, position: s.position, logical: s.logical })
        .collect();
    let Ok(Reply::Windows { windows }) = Client::connect().and_then(|mut c| c.request(&Request::ListWindows)) else { return all };
    for w in windows.into_iter().filter(|w| !w.minimized && w.app_id != APP_ID) {
        let (cw, ch) = w.content.as_ref().map_or((0, 0), |r| (r.width.max(0), r.height.max(0)));
        let label = if w.title.is_empty() { app_name(&w.app_id) } else { w.title.clone() };
        all.push(Shareable {
            key: format!("window:{}", w.id),
            label,
            window: true,
            source: capture::Source::Window { id: w.id, app_id: w.app_id, title: w.title },
            size: (cw as u32, ch as u32),
            position: w.content.as_ref().map_or((0, 0), |r| (r.x, r.y)),
            logical: (cw, ch),
        });
    }
    all
}

/// What people call the app `app_id` ("" if unknown).
pub fn app_name(app_id: &str) -> String {
    olo_xdg::desktop_entry::find(app_id).map(|e| e.name).unwrap_or_default()
}

struct Portal {
    pickers: HashMap<WindowId, picker::Picker>,
    /// Thumbnails come before their window has drawn: kept until it does.
    thumbs: Vec<picker::Thumb>,
    /// The bus, once served (to end sessions from here).
    bus: std::sync::Arc<std::sync::Mutex<Option<zbus::blocking::Connection>>>,
}

impl Portal {
    /// End a session from our side, off the UI thread (it waits on the bus).
    fn end(&self, session: String) {
        let Some(conn) = self.bus.lock().unwrap().clone() else { return };
        let _ = std::thread::Builder::new().name("portal-end".into()).spawn(move || portal::end(&conn, &session));
    }
}

impl Portal {
    fn close(&mut self, cx: &mut Cx, win: WindowId, answer: Option<Vec<String>>) {
        if let Some(p) = self.pickers.remove(&win) {
            p.answer(answer);
        }
        cx.close_window(win);
    }
}

impl App for Portal {
    fn ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut egui::Ui) {
        let Some(p) = self.pickers.get_mut(&win) else { return };
        for t in std::mem::take(&mut self.thumbs) {
            if t.window == win {
                p.thumb(ui.ctx(), t);
            } else {
                self.thumbs.push(t);
            }
        }
        if let Some(answer) = p.ui(ui) {
            self.close(cx, win, answer);
        }
    }

    fn close_requested(&mut self, cx: &mut Cx, win: WindowId) {
        self.close(cx, win, None);
    }

    /// Sharing (or asking to): an upgrade would cut it off, so it waits.
    fn busy(&self) -> bool {
        stream::live() > 0 || !self.pickers.is_empty()
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<instance::Launch>() {
            // `olo-portal --stop SESSION` (Deskbar's Stop Sharing); any
            // other second start: we are already serving the session.
            Ok(l) => {
                if let [flag, session] = l.args.as_slice()
                    && flag == "--stop"
                {
                    self.end(session.clone());
                }
                return;
            }
            Err(other) => other,
        };
        let message = match message.downcast::<picker::Thumb>() {
            Ok(t) => {
                let win = t.window;
                self.thumbs.push(*t);
                cx.redraw(win);
                return;
            }
            Err(other) => other,
        };
        let Ok(m) = message.downcast::<portal::ToUi>() else { return };
        match *m {
            portal::ToUi::Pick(ask) => {
                let (mut screens, mut windows) = (Vec::new(), Vec::new());
                for s in shareables() {
                    let item = picker::Item { key: s.key, label: s.label, size: s.size, source: s.source };
                    if s.window { windows.push(item) } else { screens.push(item) }
                }
                let (win, p) = picker::Picker::open(cx, ask, screens, windows);
                self.pickers.insert(win, p);
            }
            portal::ToUi::Ended => {}
            portal::ToUi::Lost(session) => self.end(session),
            portal::ToUi::Cancel(request) => {
                let wins: Vec<WindowId> = self.pickers.iter().filter(|(_, p)| p.request() == request).map(|(w, _)| *w).collect();
                for win in wins {
                    self.close(cx, win, None);
                }
            }
        }
    }
}

fn usage() -> std::process::ExitCode {
    eprintln!("usage: olo-portal --service | --bus ADDRESS | capture-test OUT.png [OUTPUT] | stream-test [OUTPUT|-] [SECONDS] | share-test OUT.png [--bus ADDRESS] [--hold SECS] [--types 1|2|3] | --stop SESSION");
    std::process::ExitCode::FAILURE
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fail = |e: String| {
        eprintln!("olo-portal: {e}");
        std::process::ExitCode::FAILURE
    };
    // The tests' logs (the service's are the kit's).
    let logs = || {
        let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    };
    if args.first().is_some_and(|a| a.ends_with("-test")) {
        logs();
    }
    let address = match args.first().map(String::as_str) {
        Some("capture-test") => return capture_test(args.get(1).map(String::as_str), args.get(2).map(String::as_str)).map_or_else(fail, |()| std::process::ExitCode::SUCCESS),
        Some("stream-test") => {
            let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
            return match stream::run(capture::Source::Output(args.get(1).cloned().filter(|o| o != "-")), true, || {}) {
                Ok(share) => {
                    println!("node {} ({}×{})", share.node, share.size.0, share.size.1);
                    std::thread::sleep(std::time::Duration::from_secs(secs));
                    share.stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    std::process::ExitCode::SUCCESS
                }
                Err(e) => fail(e),
            };
        }
        Some("share-test") => {
            let Some(out) = args.get(1) else { return usage() };
            let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str);
            let hold = value("--hold").and_then(|s| s.parse().ok()).unwrap_or(0);
            // What it asks for: 1 screens, 2 windows, 3 either.
            let types = value("--types").and_then(|s| s.parse().ok()).unwrap_or(1);
            return client::share_test(out, value("--bus"), hold, types).map_or_else(fail, |()| std::process::ExitCode::SUCCESS);
        }
        Some("--service") => None,
        // Deskbar's Stop Sharing, to the running service.
        Some("--stop") if args.len() == 2 => {
            return if instance::forward(APP_ID, &args) {
                std::process::ExitCode::SUCCESS
            } else {
                eprintln!("olo-portal: not running");
                std::process::ExitCode::FAILURE
            };
        }
        Some("--bus") => match args.get(1) {
            Some(a) => Some(a.clone()),
            None => return usage(),
        },
        _ => return usage(),
    };
    // Only the real session's backend answers the session's apps: a test
    // or nested session must never take the user's portal over.
    if address.is_none() && std::env::var_os("OLO_REAL_SESSION").is_none() {
        eprintln!("olo-portal: not in a real Olo session (use --bus ADDRESS to try it)");
        return std::process::ExitCode::FAILURE;
    }
    if address.is_none() && instance::forward(APP_ID, &args) {
        return std::process::ExitCode::SUCCESS;
    }
    olo_kit::run(APP_ID, move |cx| {
        cx.stay_resident();
        // (`--stop` comes this way, in a test's session too.)
        instance::listen(APP_ID, cx.proxy());
        let proxy = cx.proxy();
        let bus = std::sync::Arc::new(std::sync::Mutex::new(None));
        let slot = bus.clone();
        std::thread::Builder::new()
            .name("portal-bus".into())
            .spawn(move || {
                let to_ui = proxy.clone();
                match portal::serve(address.as_deref(), move |m| to_ui.send(m)) {
                    // Serve until the app exits.
                    Ok(conn) => {
                        *slot.lock().unwrap() = Some(conn);
                        loop {
                            std::thread::park();
                        }
                    }
                    Err(e) => {
                        eprintln!("olo-portal: can't serve {}: {e}", portal::NAME);
                        std::process::exit(1);
                    }
                }
            })
            .expect("thread");
        Portal { pickers: HashMap::new(), thumbs: Vec::new(), bus }
    });
    std::process::ExitCode::SUCCESS
}

fn capture_test(out: Option<&str>, output: Option<&str>) -> Result<(), String> {
    let out = out.ok_or("capture-test needs a file to write")?;
    // (`window:ID` for a window.)
    let source = match output.and_then(|o| o.strip_prefix("window:")).and_then(|id| id.parse().ok()) {
        Some(id) => capture::Source::Window { id, app_id: String::new(), title: String::new() },
        None => capture::Source::Output(output.map(str::to_owned)),
    };
    let mut c = capture::Capture::new(&source, true)?;
    let started = std::time::Instant::now();
    let p = c.frame()?;
    let first = started.elapsed();
    client::write_png(out, p.width, p.height, p.stride as usize, &p.data)?;
    // A second, as a stream would: it comes once the picture changes.
    let again = std::time::Instant::now();
    c.request()?;
    let next = loop {
        if let Some(done) = c.poll()? {
            done?;
            break format!("the next in {:?}", again.elapsed());
        }
        if again.elapsed() > std::time::Duration::from_secs(3) {
            break "no change in 3 s".to_owned();
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    eprintln!("{}×{} {:?} ({}): first frame in {first:?}, {next}", p.width, p.height, p.format, if c.on_gpu() { "dmabuf" } else { "shared memory" });
    Ok(())
}
