//! Terminal ▸ Terminal Daemon…: how olo-termd (which runs the shells)
//! is doing, the shells it has, and a way to restart it: in place, keeping
//! the shells, or, when it has stopped answering, from scratch.

use std::time::{Duration, Instant};

use olo_kit_base::Proxy;
use olo_ui::{
    gpui::{AnyElement, Entity, IntoElement, ParentElement as _, Styled as _, Window, div, px},
    metrics as m,
    widgets::{self, Button, Column, ColumnListView, ListModel, StringView},
};

use crate::{
    Terminal, Win, client,
    ui::{Cx, WinOps as _, WindowId, WindowOptions},
    wire,
};

pub const WIDTH: u32 = 470;
pub const HEIGHT: u32 = 320;
/// How often to look again while the window is open.
const EVERY: Duration = Duration::from_secs(2);

/// Answers from the worker threads.
pub enum Report {
    Status(Result<wire::Status, String>),
    /// A restart or stop has finished (or failed).
    Done(Result<(), String>),
}

pub struct DaemonWin {
    status: Option<Result<wire::Status, String>>,
    /// When we last asked, and whether the answer is still to come.
    asked: Option<Instant>,
    waiting: bool,
    /// What we're doing ("Restarting…"), or what went wrong.
    note: String,
    busy: bool,
    list: Entity<ListModel>,
}

pub enum Action {
    /// Stop it and start it afresh, ending the shells (after asking).
    ConfirmStartOver,
    /// Bring this shell's window forward, or open one on it.
    Show(wire::ShellInfo),
}

pub fn open(cx: &mut Cx) -> (WindowId, DaemonWin) {
    let list = ListModel::new(cx, false);
    let id = cx.open_window(WindowOptions::new("Terminal Daemon", WIDTH, HEIGHT).min_size(WIDTH, 240));
    (id, DaemonWin { status: None, asked: None, waiting: false, note: String::new(), busy: false, list })
}

/// Run `work` on a thread and report back.
fn run(proxy: Proxy, work: impl FnOnce() -> Report + Send + 'static) {
    std::thread::spawn(move || proxy.send(work()));
}

fn uptime(started: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(started);
    let s = now.saturating_sub(started);
    match s {
        0..60 => "under a minute".into(),
        60..3600 => format!("{} min", s / 60),
        3600..86400 => format!("{} h {} min", s / 3600, s % 3600 / 60),
        _ => format!("{} days", s / 86400),
    }
}

impl DaemonWin {
    pub fn report(&mut self, report: Report) {
        match report {
            Report::Status(s) => {
                self.status = Some(s);
                self.waiting = false;
            }
            Report::Done(r) => {
                self.busy = false;
                self.note = r.err().unwrap_or_default();
                // Look again now.
                self.asked = None;
            }
        }
    }

    /// Confirmed: stop it, however it is, and start it afresh.
    pub fn start_over(&mut self, proxy: Proxy) {
        self.busy = true;
        self.note = "Starting over…".into();
        let known = match &self.status {
            Some(Ok(s)) => Some(s.pid),
            _ => None,
        };
        run(proxy, move || {
            Report::Done(client::stop(known).and_then(|()| {
                client::forget_kept();
                client::start()
            }))
        });
    }

    /// Restart in place, or start it if it isn't running.
    fn restart(&mut self, proxy: Proxy) {
        self.busy = true;
        if matches!(self.status, Some(Ok(_))) {
            self.note = "Restarting…".into();
            run(proxy, || {
                let r = client::restart();
                // Give it a moment to come back.
                std::thread::sleep(Duration::from_millis(500));
                Report::Done(r)
            });
        } else {
            self.note = "Starting…".into();
            run(proxy, || Report::Done(client::start()));
        }
    }
}

impl Terminal {
    /// The Terminal Daemon window: olo-termd's state and its shells, looked
    /// at again every couple of seconds while it's open.
    pub(crate) fn render_daemon(&mut self, id: WindowId, _: &mut Window, cx: &mut Cx) -> AnyElement {
        let proxy = cx.proxy();
        let Some(Win::Daemon(d)) = self.windows.get_mut(&id) else { return div().into_any_element() };
        if !d.waiting && !d.busy && d.asked.is_none_or(|t| t.elapsed() >= EVERY) {
            d.waiting = true;
            d.asked = Some(Instant::now());
            run(proxy, || Report::Status(client::status()));
            // (Drawn again with the answer; asked again after that.)
            cx.spawn(async move |me, cx| {
                cx.background_executor().timer(EVERY).await;
                let _ = me.update(cx, |_, cx| cx.redraw(id));
            })
            .detach();
        }
        let ours = std::process::id() as i32;
        let (headline, detail) = match &d.status {
            None => ("Checking…".to_owned(), String::new()),
            Some(Err(e)) => (format!("olo-termd: {e}"), "New shells can't start until it's running again.".to_owned()),
            Some(Ok(s)) => {
                let here = s.shells.iter().filter(|sh| sh.client == Some(ours)).count();
                let shells = match s.shells.len() {
                    1 => "1 shell".to_owned(),
                    n => format!("{n} shells"),
                };
                (format!("olo-termd is running (process {}, up {}).", s.pid, uptime(s.started)), format!("{shells}, {here} shown in this Terminal."))
            }
        };
        let shells: Vec<wire::ShellInfo> = match &d.status {
            Some(Ok(s)) => s.shells.clone(),
            _ => Vec::new(),
        };
        let columns = vec![Column::new("Shell", 230.0), Column::new("Process", 70.0).right(), Column::new("Shown", 110.0)];
        let cells = shells.clone();
        let show = cx.listener(move |t, row: &usize, _, cx| {
            if let Some(sh) = shells.get(*row).cloned() {
                t.daemon_action(cx, id, Action::Show(sh));
            }
        });
        let list = ColumnListView::new("termd-shells", &d.list, columns, cells.len(), move |cell, window, cx| {
            let Some(sh) = cells.get(cell.row) else { return };
            let text = match cell.column {
                0 => sh.title.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or("Shell").to_owned(),
                1 => sh.pid.to_string(),
                _ => match sh.client {
                    Some(p) if p == ours => "Here",
                    Some(_) => "Elsewhere",
                    None => "Not shown",
                }
                .to_owned(),
            };
            cell.text(window, cx, text);
        })
        .on_open(move |row, window, cx| show(&row, window, cx));
        let running = matches!(d.status, Some(Ok(_)));
        let answering = !matches!(&d.status, Some(Err(e)) if e.starts_with("Not responding"));
        // (One that isn't answering can only start over.)
        let label = if running || !answering { "Restart" } else { "Start" };
        let can_restart = !d.busy && answering && d.status.is_some();
        let can_start_over = !d.busy && (running || !answering);
        let note = d.note.clone();
        let restart = cx.listener(move |t, _: &(), _, cx| {
            let proxy = cx.proxy();
            if let Some(Win::Daemon(d)) = t.windows.get_mut(&id) {
                d.restart(proxy);
            }
            cx.redraw(id);
        });
        let over = cx.listener(move |t, _: &(), _, cx| t.daemon_action(cx, id, Action::ConfirmStartOver));
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .child(widgets::heading(headline))
            .child(StringView::new(detail).dim(true))
            .child(div().h(px(m::SPACING - m::TIGHT)))
            .child(div().flex_1().min_h(px(60.0)).child(list))
            .child(div().h(px(m::SPACING - m::TIGHT)))
            .child(
                div().flex().flex_row().items_center().child(div().flex_1().child(StringView::new(note).dim(true))).child(
                    widgets::button_row()
                        .child(Button::new("Start Over…").enabled(can_start_over).on_click(move |window, cx| over(&(), window, cx)))
                        .child(Button::new(label).default(true).enabled(can_restart).on_click(move |window, cx| restart(&(), window, cx))),
                ),
            )
            .into_any_element()
    }
}
