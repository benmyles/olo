//! The Clients window (Pool ▸ Clients): the pool's activity over time, a
//! line each for Claude Code and Codex (requests a minute, over the last
//! hour, six hours or day); Codex's service, the daemon Codex's windows
//! run their conversations in (its version, whether it is on the pool's
//! settings, where its connections go, its conversations), with Restart;
//! and each Claude Code and Codex running, and where its connections go.
//! What's running is read every two seconds while the window is open, off
//! the UI thread (`running::scan`); a restart is Codex's own, on a worker
//! thread too.

use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use olo_kit::{
    Cx, WindowId, WindowOptions,
    dashboard::{self, LegendItem},
    egui,
    graph::{Graph, Series},
    metrics as m,
    style::{self, c},
    theme::Hue,
    widgets::{self, Button, MenuField, ScrollView, StringView},
};
use olo_token_pool::{
    gateway::Shared,
    model::{Provider, now},
    running::{Process, Running},
};

/// How often what's running is read.
const EVERY: Duration = Duration::from_secs(2);
const GRAPH_HEIGHT: f32 = 110.0;

/// The spans the chart shows: its name, minutes, minutes to a step, and
/// seconds between time lines.
const SPANS: [(&str, i64, i64, f64); 3] = [("Last Hour", 60, 1, 600.0), ("Last 6 Hours", 360, 5, 3600.0), ("Last 24 Hours", 1440, 15, 4.0 * 3600.0)];

pub struct ClientsWindow {
    pub win: WindowId,
    span: usize,
    running: Option<Running>,
    restarting: bool,
    /// What the last restart came to (error?).
    note: Option<(String, bool)>,
    watching: Arc<AtomicBool>,
}

/// What the person asked of the window.
pub enum Action {
    Restart,
}

/// "~/src/olo" for a folder in the home folder.
fn folder(p: &Path) -> String {
    match std::env::var_os("HOME").map(std::path::PathBuf::from).and_then(|h| p.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}

/// `s` with a capital.
fn sentence(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn hue(client: &str) -> Hue {
    match client {
        "Claude Code" => Hue::Orange,
        "Codex" => Hue::Blue,
        _ => Hue::Gray,
    }
}

impl ClientsWindow {
    /// Open the window, and read what's running (`found`) until it
    /// closes.
    pub fn open(cx: &mut Cx, proxy_port: u16, found: impl Fn(Running) + Send + 'static) -> ClientsWindow {
        let win = cx.open_window(WindowOptions::new("Clients", 480, 600).min_size(400, 420));
        let watching = Arc::new(AtomicBool::new(true));
        let w = watching.clone();
        let _ = std::thread::Builder::new().name("pool-clients".into()).spawn(move || {
            while w.load(Ordering::Relaxed) {
                found(olo_token_pool::running::scan(proxy_port));
                std::thread::sleep(EVERY);
            }
        });
        ClientsWindow { win, span: 0, running: None, restarting: false, note: None, watching }
    }

    pub fn close(&self) {
        self.watching.store(false, Ordering::Relaxed);
    }

    pub fn found(&mut self, r: Running) {
        self.running = Some(r);
    }

    pub fn restarted(&mut self, r: Result<i32, String>) {
        self.restarting = false;
        self.note = Some(match r {
            Ok(pid) => (format!("Restarted (now pid {pid})."), false),
            Err(e) => (e, true),
        });
    }

    /// The window's content: which of `uses` (Claude Code, Codex) are set
    /// to use the pool says whether going straight out is wrong.
    pub fn ui(&mut self, ui: &mut egui::Ui, shared: &Shared, uses: Option<[bool; 2]>) -> Option<Action> {
        let mut action = None;
        widgets::window_content(ui, |ui| {
            self.activity(ui, shared);
            widgets::separator(ui);
            let list = egui::Rect::from_min_size(ui.cursor().min, ui.available_size());
            ScrollView::new("pool-clients").border(false).show_in(ui, list, |ui, _| {
                ui.spacing_mut().item_spacing.y = m::TIGHT;
                action = self.service(ui, uses.is_some_and(|u| u[1]));
                ui.add_space(m::SPACING);
                self.processes(ui, uses);
            });
            ui.advance_cursor_after_rect(list);
        });
        action
    }

    /// The chart: requests a minute by client, with their totals below.
    fn activity(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        let p = style::palette(ui.ctx());
        let (name, span, step, grid) = SPANS[self.span];
        ui.horizontal(|ui| {
            widgets::heading(ui, "Activity");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let names = SPANS.map(|s| s.0);
                ui.add(MenuField::new("pool-span", &mut self.span, &names).width(140.0));
            });
        });
        let t = now();
        let since = t - span * 60;
        let (clients, lines, totals) = shared.activity(|a| {
            let clients: Vec<String> = a.clients();
            let lines: Vec<VecDeque<(f64, f64)>> = clients.iter().map(|cl| a.series(cl, t, span, step)).collect();
            let totals: Vec<_> = clients.iter().map(|cl| a.totals(cl, since)).collect();
            (clients, lines, totals)
        });
        // Claude Code and Codex always; others once they've been seen.
        let shown: Vec<usize> = (0..clients.len()).filter(|&i| i < 2 || totals[i].requests > 0).collect();
        let series: Vec<Series> = shown.iter().map(|&i| Series::new(&clients[i], c(p.hue(hue(&clients[i]))), &lines[i]).fill(true)).collect();
        let fmt = |v: f64| if v >= 10.0 || v == 0.0 { format!("{v:.0}/min") } else { format!("{v:.1}/min") };
        let r = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), GRAPH_HEIGHT));
        Graph::new(("pool-activity", name), &series, t as f64).span((span * 60) as f64).grid(grid).unit(&fmt).smooth(false).show(ui, r);
        ui.advance_cursor_after_rect(r);
        ui.add_space(m::TIGHT);
        let values: Vec<String> = shown
            .iter()
            .map(|&i| {
                let x = totals[i];
                let mut v = format!("{} request{}", x.requests, if x.requests == 1 { "" } else { "s" });
                if x.failed > 0 {
                    v.push_str(&format!(", {} failed", x.failed));
                }
                if x.input_tokens + x.output_tokens > 0 {
                    v.push_str(&format!(" · {} in, {} out", crate::tokens(x.input_tokens), crate::tokens(x.output_tokens)));
                }
                v
            })
            .collect();
        let items: Vec<LegendItem> = shown.iter().zip(values).map(|(&i, value)| LegendItem { color: c(p.hue(hue(&clients[i]))), name: &clients[i], value }).collect();
        let w = ui.available_width();
        let h = dashboard::legend_height(ui, w, &items);
        let r = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(w, h));
        dashboard::legend(ui, r, &items);
        ui.advance_cursor_after_rect(r);
    }

    /// Codex's service: what it is, and Restart.
    fn service(&mut self, ui: &mut egui::Ui, should_use: bool) -> Option<Action> {
        let p = style::palette(ui.ctx());
        widgets::heading(ui, "Codex’s Service");
        let t = now();
        let mut action = None;
        let Some(running) = &self.running else {
            ui.add(StringView::new("Looking…").dim(true));
            return None;
        };
        let Some(d) = &running.daemon else {
            ui.add(StringView::new("Not running: Codex starts it with its first window.").dim(true));
            return None;
        };
        let version = d.version.as_deref().map(|v| format!("Codex {v}")).unwrap_or_else(|| "Codex".into());
        ui.add(StringView::new(&format!("{version} · since {} ({}) · pid {}", crate::clock(d.started), crate::span(t - d.started), d.pid)));
        let (route, wrong) = if d.stale {
            ("Started before the pool’s settings last changed, so still on the ones before (restarted onto them in a moment)".to_owned(), true)
        } else {
            let route = match (d.connections.pool, d.connections.direct) {
                (n, 0) if n > 0 => format!("through the pool ({n} connection{})", if n == 1 { "" } else { "s" }),
                _ => d.connections.describe(Provider::Codex),
            };
            (format!("On the current settings · {route}"), should_use && d.connections.direct > 0 && d.connections.pool == 0)
        };
        let color = if wrong { c(p.failure) } else { c(p.panel_text) };
        ui.add(StringView::new(&route).wrap(true).color(color));
        match &d.threads {
            Ok(threads) if threads.is_empty() => {
                ui.add(StringView::new("No conversations open.").dim(true));
            }
            Ok(threads) => {
                for th in threads {
                    let state = match (th.working, th.waiting) {
                        (true, true) => "waiting for you",
                        (true, false) => "working",
                        _ => "idle",
                    };
                    let title = if th.title.is_empty() { "A conversation".to_owned() } else { format!("“{}”", th.title) };
                    let place = th.cwd.as_deref().map(|d| format!(" in {}", folder(d))).unwrap_or_default();
                    ui.add(StringView::new(&format!("{title}: {state}{place}")).wrap(true));
                }
            }
            Err(e) => {
                ui.add(StringView::new(&format!("Its conversations: it didn’t say ({e}).")).wrap(true).dim(true));
            }
        }
        ui.add_space(m::TIGHT);
        ui.horizontal(|ui| {
            let label = if self.restarting { "Restarting…" } else { "Restart" };
            if ui.add_enabled(!self.restarting, Button::new(label)).clicked() {
                self.restarting = true;
                self.note = None;
                action = Some(Action::Restart);
            }
            match &self.note {
                Some((text, error)) => {
                    let color = if *error { c(p.failure) } else { style::disabled_text(&p) };
                    ui.add(StringView::new(text).color(color));
                }
                None => {
                    ui.add(StringView::new("Ends a turn going on; Codex’s windows reconnect.").dim(true));
                }
            }
        });
        action
    }

    /// Each Claude Code and Codex running, and where it connects.
    fn processes(&self, ui: &mut egui::Ui, uses: Option<[bool; 2]>) {
        let p = style::palette(ui.ctx());
        let t = now();
        widgets::heading(ui, "Running");
        let Some(running) = &self.running else { return };
        for (k, (prov, list)) in [(Provider::Claude, &running.claude), (Provider::Codex, &running.codex)].into_iter().enumerate() {
            ui.add_space(m::TIGHT);
            let title = match list.len() {
                0 => format!("{}: none", prov.client()),
                n => format!("{} ({n})", prov.client()),
            };
            ui.add(StringView::new(&title).dim(list.is_empty()));
            let should_use = uses.is_some_and(|u| u[k]);
            for proc in list {
                let Process { pid, started, cwd, connections } = proc;
                let place = cwd.as_deref().map(folder).unwrap_or_else(|| "?".into());
                let line = format!("{} · since {} ({}) · pid {pid}", sentence(&connections.describe(prov)), crate::clock(*started), crate::span(t - started));
                // Codex's windows talk to its service; only Claude Code's
                // own connections say where its requests go (and some go
                // straight out on purpose: an MCP server).
                let wrong = should_use && connections.direct > 0 && connections.pool == 0 && prov == Provider::Claude;
                let color = if wrong { c(p.failure) } else { style::disabled_text(&p) };
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.add_space(m::SPACING);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.add(StringView::new(&place));
                        ui.add(StringView::new(&line).color(color));
                    });
                });
            }
        }
    }
}
