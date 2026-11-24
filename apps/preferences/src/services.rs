//! Services: every service of Olo's session, how each stands, and
//! Restart, Stop, Start and Show Log for the chosen one (Ben, 2026-11-23,
//! on asking whether Passwords' SSH agent had to be restarted by hand to
//! take a fix: "a Preferences -> Services app that shows all of the olo
//! services and their status and lets me manage/restart them").
//!
//! The session tells of them and does what's asked
//! (`olo_protocol::services`, over its socket; `olo-session services`
//! for a terminal or an agent). The list follows it live: the session
//! sends it again as one changes, and nothing is asked while nothing
//! does. One still running an older program than the one installed (an
//! upgrade it hasn't taken) says so, and Restart is the cure.

use std::{
    io::{BufRead as _, BufReader, Write as _},
    os::unix::net::UnixStream,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use olo_kit::{
    ParentElement as _, SharedString, Styled as _, div, metrics as m, point, px, style, theme,
    widgets::{Button, Column, ColumnListView, EmptyState, LIGHT, Light, ListModel, StringView},
    words,
};
use olo_protocol::services::{Ask, Said, Service, State as Standing};

use crate::{Msg, PanelView, form::Ui, panel};

/// What the pane keeps: the list's selection, by program (the list
/// changes under it).
pub struct State {
    list: ListModel,
    selected: Option<String>,
}

impl State {
    pub fn new() -> State {
        // (Its link to the session made as the pane opens.)
        let _ = link();
        State { list: ListModel::new(false), selected: None }
    }
}

/// What the session last said, kept for the window.
#[derive(Default)]
struct Shared {
    /// The list, while the session is connected.
    services: Option<Vec<Service>>,
    /// What went wrong last with something asked of it.
    error: Option<String>,
}

struct Link {
    shared: Arc<Mutex<Shared>>,
}

fn socket() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    olo_protocol::services::socket_path(&runtime, &olo_xdg::display_name())
}

fn link() -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        if cfg!(test) {
            return Link { shared: Arc::new(Mutex::new(Shared { services: Some(test_services()), error: None })) };
        }
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (sh, out) = (shared.clone(), olo_kit::proxy());
        let _ = std::thread::Builder::new().name("services-watch".into()).spawn(move || follow(&sh, &out));
        Link { shared }
    })
}

/// The session's list, followed: told each change while connected; when
/// the connection ends (the session re-executing for an upgrade), again
/// as soon as its socket is made again, its folder watched meanwhile.
fn follow(shared: &Arc<Mutex<Shared>>, out: &olo_kit::Proxy) {
    let path = socket();
    loop {
        if let Ok(stream) = UnixStream::connect(&path) {
            let _ = (&stream).write_all(b"{\"do\":\"watch\"}\n");
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                if let Ok(Said::Services { services }) = serde_json::from_str::<Said>(&line) {
                    shared.lock().unwrap_or_else(|e| e.into_inner()).services = Some(services);
                    out.send(Msg::Redraw);
                }
            }
        }
        shared.lock().unwrap_or_else(|e| e.into_inner()).services = None;
        out.send(Msg::Redraw);
        // Until the socket is there again (made, or replaced): the kernel
        // says so. (An hour at most, should it never say: then tried.)
        let (tx, rx) = std::sync::mpsc::channel();
        let watch = olo_kit::watch::watch([path.clone()], move |_| {
            let _ = tx.send(());
        });
        if UnixStream::connect(&path).is_err() {
            let _ = rx.recv_timeout(if watch.is_some() { Duration::from_secs(3600) } else { Duration::from_secs(30) });
        }
        drop(watch);
    }
}

/// `ask` of the session, off the window's thread; what went wrong kept
/// to say. The list itself comes back by the watch.
fn ask(ask: Ask) {
    let shared = link().shared.clone();
    let out = olo_kit::proxy();
    let _ = std::thread::Builder::new().name("services-ask".into()).spawn(move || {
        let said = (|| -> std::io::Result<Said> {
            let mut stream = UnixStream::connect(socket())?;
            stream.set_read_timeout(Some(Duration::from_secs(10)))?;
            let mut line = serde_json::to_vec(&ask).map_err(std::io::Error::other)?;
            line.push(b'\n');
            stream.write_all(&line)?;
            let mut reply = String::new();
            BufReader::new(stream).read_line(&mut reply)?;
            serde_json::from_str(&reply).map_err(std::io::Error::other)
        })();
        let error = match said {
            Ok(Said::Services { .. }) => None,
            Ok(Said::Error { message }) => Some(message),
            Err(e) => Some(olo_kit::tr!("The session didn’t answer: {}", e)),
        };
        shared.lock().unwrap_or_else(|e| e.into_inner()).error = error;
        out.send(Msg::Redraw);
    });
}

/// Test data (the layout lint): every kind of standing, names as long as
/// the real ones; never the session.
fn test_services() -> Vec<Service> {
    // (Started three hours ago; no process, so nothing of this computer's
    // is read for them.)
    let hours_ago = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()).saturating_sub(3 * 3600);
    let s = |program: &str, args: &[&str], state: Standing| Service {
        program: program.into(),
        args: args.iter().map(|a| a.to_string()).collect(),
        pid: 0,
        since: if state == Standing::Running { hours_ago } else { 0 },
        restarts: 0,
        state,
        window_server: program == "olo-app-server",
    };
    vec![
        s("olo-app-server", &[], Standing::Running),
        s("olo-deskbar", &[], Standing::Running),
        s("olo-tracker", &["--desktop"], Standing::Running),
        s("olo-polkit-agent", &[], Standing::Running),
        s("olo-notifications", &[], Standing::Running),
        s("olo-pool", &["--service"], Standing::Stopped),
        s("olo-phoned", &[], Standing::GaveUp),
        s("olo-passwords", &["--agent"], Standing::NotWanted),
        s("olo-calendar", &["--service"], Standing::Running),
    ]
}

/// How a service stands, as its row says it.
struct Row {
    name: SharedString,
    light: Light,
    said: SharedString,
    started: SharedString,
    memory: SharedString,
    /// Running an older program than the one installed.
    behind: bool,
}

/// Its process's resident memory, from the kernel.
fn memory(pid: i32) -> Option<u64> {
    let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: sysconf has no preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
    Some(pages * page)
}

/// Its program was replaced since it started (an upgrade installed a new
/// one): the kernel names the one it runs "(deleted)".
fn behind(pid: i32) -> bool {
    std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|p| p.to_string_lossy().ends_with(" (deleted)"))
}

/// When it started, as the clock says it (as Deskbar's, 12 or 24
/// hours): the time today, the day and time before. (A clock time, not
/// an age: a session's services all start together, and an age said
/// nothing a row's neighbours didn't; nor does it change as it's shown.)
fn started(since: u64, clock: &olo_protocol::ClockSettings) -> String {
    // SAFETY: localtime_r fills the zeroed tm we own.
    let at = |secs: u64| unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let t = secs as libc::time_t;
        libc::localtime_r(&t, &mut tm);
        tm
    };
    let (tm, today) = (at(since), at(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())));
    let plain = olo_protocol::ClockSettings { show_seconds: false, show_day: (tm.tm_yday, tm.tm_year) != (today.tm_yday, today.tm_year), ..clock.clone() };
    plain.format(tm.tm_hour as u32, tm.tm_min as u32, 0, tm.tm_wday as u32)
}

fn row(s: &Service, clock: &olo_protocol::ClockSettings) -> Row {
    let name = words::of_text(olo_protocol::services::about(&s.program).0);
    let running = s.state == Standing::Running && s.pid > 0;
    let behind = running && behind(s.pid);
    let (light, said): (Light, SharedString) = match s.state {
        // (What to do, at a glance down the list: Opus's review.)
        Standing::Running if behind => (Light::Waiting, words::tr("Restart to update").into()),
        Standing::Running if s.restarts > 0 => (Light::On, words::count(u64::from(s.restarts), "Running, started again once", "Running, started again {n} times").into()),
        Standing::Running => (Light::On, words::tr("Running").into()),
        Standing::Stopped => (Light::Off, words::tr("Stopped").into()),
        Standing::NotWanted => (Light::Off, words::tr("Turned off").into()),
        Standing::GaveUp => (Light::Down, words::tr("Stopped after ending too often").into()),
    };
    // (A dash where there's nothing to say: a blank read as a fault.)
    let started: SharedString = if s.since > 0 && s.state == Standing::Running { started(s.since, clock).into() } else { "–".into() };
    // (Whole megabytes: tenths changed at every look, and a still list
    // flickered.)
    let memory = if running { memory(s.pid).map(|b| olo_kit::tr!("{} MB", (b as f64 / 1_000_000.0).round() as u64)).unwrap_or_else(|| "–".into()) } else { "–".into() };
    Row { name, light, said, started, memory: memory.into(), behind }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let (services, error) = {
        let s = link().shared.lock().unwrap_or_else(|e| e.into_inner());
        (s.services.clone(), s.error.clone())
    };
    let Some(services) = services else {
        ui.add(div().flex_1().flex().child(EmptyState::new("Olo’s session isn’t answering.").icon("process")));
        return;
    };
    let clock = ui.s.clock.clone();
    let rows: Rc<Vec<Row>> = Rc::new(services.iter().map(|s| row(s, &clock)).collect());
    // (The chosen one by its program; the first while none is.)
    let chosen = st.selected.as_ref().and_then(|p| services.iter().position(|s| &s.program == p)).unwrap_or(0);
    if st.list.state().single() != Some(chosen) {
        st.list.state_mut().select(chosen);
    }
    let programs: Vec<String> = services.iter().map(|s| s.program.clone()).collect();
    let (cells, names) = (rows.clone(), rows.clone());
    let columns = vec![Column::new("Service", 210.0), Column::new("Status", 190.0), Column::new("Started", 130.0), Column::new("Memory", 100.0).right()];
    let list = ColumnListView::new(ui.cx, "services", &st.list, columns, rows.len(), move |_: &PanelView, cell, g| {
        let r = &cells[cell.row];
        match cell.column {
            0 => cell.text(g, r.name.clone()),
            1 => {
                // (The light, then the words after it.)
                let b = cell.bounds;
                let at = point(b.left() + px(m::TEXT_INSET + LIGHT / 2.0), b.center().y);
                style::dot(g, at, LIGHT, r.light.color(&theme::palette()));
                let shift = px(LIGHT + m::TIGHT);
                cell.bounds.origin.x += shift;
                cell.bounds.size.width -= shift;
                cell.text(g, r.said.clone());
            }
            2 => cell.text(g, r.started.clone()),
            _ => cell.text(g, r.memory.clone()),
        }
    })
    .type_select(move |_, i| names[i].name.clone())
    .on_select(ui.on_view(move |v, _: &(), cx| {
        if let panel::State::Services(st) = &mut v.state {
            st.selected = st.list.state().single().and_then(|i| programs.get(i).cloned());
            cx.notify();
        }
    }));
    ui.add(div().flex_1().min_h(px(m::LIST_ROW * 6.0)).flex().flex_col().child(div().flex_1().min_h_0().child(list)));

    // The chosen one: what it's for, and what can be done with it. As tall
    // whichever is chosen (a line for a note kept, empty or not), so the
    // list and the buttons never move as the choice does (Opus's review).
    let Some(s) = services.get(chosen).cloned() else { return };
    let r = &rows[chosen];
    let (_, what) = olo_protocol::services::about(&s.program);
    let note: Option<(Option<Light>, SharedString)> = if r.behind {
        Some((Some(Light::Waiting), olo_kit::tr!("A newer version was installed after {} started. Restart it to use the new one.", r.name).into()))
    } else if s.window_server {
        Some((None, words::tr("The Window Server can’t be stopped, only restarted.").into()))
    } else {
        None
    };
    let note = div().min_h(px(m::LABEL_ROW)).flex().flex_row().items_center().children(note.map(|(light, said)| {
        let lit = light.is_some();
        div().flex().flex_row().items_center().gap(px(m::TIGHT)).children(light.map(olo_kit::widgets::light)).child(if lit {
            StringView::new(said).wrap(true)
        } else {
            StringView::new(said).dim(true).wrap(true)
        })
    }));
    // (What it runs, as its log and `olo-session` name it.)
    let command = std::iter::once(s.program.as_str()).chain(s.args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ");
    let process = if s.pid > 0 { olo_kit::tr!("Process {} · {}", s.pid, command) } else { command };
    let about = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(StringView::new(r.name.clone()).bold(true))
        .child(div().mt(px(m::TIGHT)).min_h(px(m::LABEL_ROW)).child(StringView::new(words::of_text(what)).wrap(true)))
        .child(div().mt(px(m::TIGHT / 2.0)).child(note))
        .child(div().mt(px(m::TIGHT)).child(StringView::new(process).size(style::SMALL_SIZE).dim(true).cut(true)));
    let program = s.program.clone();
    let show_log = ui.on_click_view(move |_, _| {
        let _ = olo_xdg::spawn_detached(&format!("olo-logs --program '{}'", program.replace('\'', "")));
    });
    let program = s.program.clone();
    let window_server = s.window_server;
    let restart = ui.on_click_view(move |_, _| {
        let program = program.clone();
        if window_server {
            // (The session's reload: every window drawn again, every app
            // reconnecting. Asked first.)
            olo_kit::widgets::open_app_alert(
                "Services",
                "warning",
                &words::tr("Restart the Window Server?\n\nThe screen goes blank for a moment while every window is drawn again; nothing you have open closes."),
                &["Restart", "Cancel"],
                move |chosen| {
                    if chosen == Some(0) {
                        ask(Ask::Restart { program: program.clone() });
                    }
                },
            );
        } else {
            ask(Ask::Restart { program });
        }
    });
    let program = s.program.clone();
    let running = s.state == Standing::Running;
    let stop_start = ui.on_click_view(move |_, _| {
        let program = program.clone();
        ask(if running { Ask::Stop { program } } else { Ask::Start { program } });
    });
    // (Show Log only shows; the two after it change what runs: set
    // apart from them.)
    let buttons = div()
        .flex()
        .flex_row()
        .flex_shrink_0()
        .gap(px(m::SPACING))
        .child(div().mr(px(m::SPACING)).child(Button::new("Show Log").id("services-log").on_click(show_log)))
        .child(Button::new(if running { "Stop" } else { "Start" }).id("services-stop").enabled(!window_server).min_width(m::BUTTON_MIN_WIDTH).on_click(stop_start))
        // (A stopped one is started, not restarted.)
        .child(Button::new("Restart").id("services-restart").enabled(running).min_width(m::BUTTON_MIN_WIDTH).on_click(restart));
    ui.add(div().flex().flex_row().items_start().gap(px(m::SECTION)).child(about).child(buttons));
    if let Some(error) = error {
        ui.add(StringView::new(error).color(style::c(olo_theme::readable(theme::palette().failure, theme::palette().panel_background, 4.5))).wrap(true));
    }
}
