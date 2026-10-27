//! Terminal ▸ Terminal Daemon…: how olo-termd (which runs the shells)
//! is doing, the shells it has, and a way to restart it: in place, keeping
//! the shells, or, when it has stopped answering, from scratch.

use std::time::{Duration, Instant};

use olo_kit::{
    Cx, Proxy, WindowId, WindowOptions,
    egui::{self, Rect, UiBuilder, vec2},
    metrics as m,
    widgets::{self, Button, Column, ColumnListView, ListState, StringView},
};

use crate::{client, wire};

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
    list: ListState,
}

pub enum Action {
    /// Stop it and start it afresh, ending the shells (after asking).
    ConfirmStartOver,
    /// Bring this shell's window forward, or open one on it.
    Show(wire::ShellInfo),
}

pub fn open(cx: &mut Cx) -> (WindowId, DaemonWin) {
    let id = cx.open_window(WindowOptions::new("Terminal Daemon", WIDTH, HEIGHT).min_size(WIDTH, 240));
    (id, DaemonWin { status: None, asked: None, waiting: false, note: String::new(), busy: false, list: ListState::default() })
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
        run(proxy, move || Report::Done(client::stop(known).and_then(|()| client::start())));
    }

    pub fn ui(&mut self, cx: &mut Cx, ui: &mut egui::Ui) -> Option<Action> {
        if !self.waiting && !self.busy && self.asked.is_none_or(|t| t.elapsed() >= EVERY) {
            self.waiting = true;
            self.asked = Some(Instant::now());
            run(cx.proxy(), || Report::Status(client::status()));
        }
        ui.ctx().request_repaint_after(EVERY);
        let ours = std::process::id() as i32;
        let mut action = None;
        widgets::window_content(ui, |ui| {
            ui.spacing_mut().item_spacing.y = m::TIGHT;
            let (headline, detail) = match &self.status {
                None => ("Checking…".to_owned(), String::new()),
                Some(Err(e)) => (format!("olo-termd: {e}"), "New shells can't start until it's running again.".to_owned()),
                Some(Ok(s)) => {
                    let here = s.shells.iter().filter(|sh| sh.client == Some(ours)).count();
                    let shells = match s.shells.len() {
                        1 => "1 shell".to_owned(),
                        n => format!("{n} shells"),
                    };
                    (
                        format!("olo-termd is running (process {}, up {}).", s.pid, uptime(s.started)),
                        format!("{shells}, {here} shown in this Terminal."),
                    )
                }
            };
            widgets::heading(ui, &headline);
            ui.add(StringView::new(&detail).dim(true));
            ui.add_space(m::SPACING - m::TIGHT);

            let shells: &[wire::ShellInfo] = match &self.status {
                Some(Ok(s)) => &s.shells,
                _ => &[],
            };
            let columns = [Column::new("Shell", 230.0), Column::new("Process", 70.0).right(), Column::new("Shown", 110.0)];
            let list = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), (ui.available_height() - m::CONTROL_HEIGHT - m::SPACING).max(60.0)));
            let resp = ui.scope_builder(UiBuilder::new().max_rect(list), |ui| {
                ColumnListView::new("termd-shells", &columns, shells.len(), &mut self.list).show(ui, |cell| {
                    let sh = &shells[cell.row];
                    match cell.column {
                        0 => cell.text(sh.title.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or("Shell")),
                        1 => cell.text(&sh.pid.to_string()),
                        _ => cell.text(match sh.client {
                            Some(p) if p == ours => "Here",
                            Some(_) => "Elsewhere",
                            None => "Not shown",
                        }),
                    }
                })
            });
            ui.advance_cursor_after_rect(list);
            if let Some(sh) = resp.inner.activated.and_then(|i| shells.get(i)) {
                action = Some(Action::Show(sh.clone()));
            }
            ui.add_space(m::SPACING - m::TIGHT);

            let running = matches!(self.status, Some(Ok(_)));
            let answering = !matches!(&self.status, Some(Err(e)) if e.starts_with("Not responding"));
            ui.horizontal(|ui| {
                ui.add(StringView::new(&self.note).dim(true));
                widgets::button_row(ui, |ui| {
                    // Restart in place, or start it if it isn't running.
                    // (One that isn't answering can only start over.)
                    let label = if running || !answering { "Restart" } else { "Start" };
                    if ui.add_enabled_ui(!self.busy && answering && self.status.is_some(), |ui| ui.add(Button::new(label).default(true))).inner.clicked() {
                        self.busy = true;
                        if running {
                            self.note = "Restarting…".into();
                            run(cx.proxy(), || {
                                let r = client::restart();
                                // Give it a moment to come back.
                                std::thread::sleep(Duration::from_millis(500));
                                Report::Done(r)
                            });
                        } else {
                            self.note = "Starting…".into();
                            run(cx.proxy(), || Report::Done(client::start()));
                        }
                    }
                    if ui.add_enabled_ui(!self.busy && (running || !answering), |ui| ui.add(Button::new("Start Over…"))).inner.clicked() {
                        action = Some(Action::ConfirmStartOver);
                    }
                });
            });
        });
        action
    }
}
