//! The Clients window (Pool ▸ Clients): the pool's activity over time, a
//! line each for Claude Code and Codex (requests a minute, over the last
//! hour, six hours or day); Codex's service, the daemon Codex's windows
//! run their conversations in (its version, whether it is on the pool's
//! settings, where its connections go, its conversations), with Restart;
//! and each Claude Code and Codex running, and where its connections go.
//! What's running is read every two seconds while the window is open, off
//! the UI thread (`running::scan`); a restart is Codex's own, on a worker
//! thread too. On olo-kit: a view on its window's own looper, told what's
//! running by its scanner itself.

use std::{
    collections::VecDeque,
    path::Path,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use olo_kit::{
    ClickEvent, Cx, Element, ParentElement as _, ScrollHandle, Styled as _, View, canvas, div, metrics as m,
    palette::Hue,
    px, style, theme,
    widgets::{self, Button, Graph, LegendItem, MenuField, ScrollView, Series, StringView, heading, separator},
};
use olo_token_pool::codex_daemon;
use olo_token_pool::{
    gateway::Shared,
    model::{Provider, now},
    running::{Process, Running},
};

use crate::ui::Msg;

/// How often what's running is read.
const EVERY: Duration = Duration::from_secs(2);
const GRAPH_HEIGHT: f32 = 110.0;

/// The spans the chart shows: its name, minutes, minutes to a step, and
/// seconds between time lines.
const SPANS: [(&str, i64, i64, f64); 3] = [("Last Hour", 60, 1, 600.0), ("Last 6 Hours", 360, 5, 3600.0), ("Last 24 Hours", 1440, 15, 4.0 * 3600.0)];

/// What the Clients window is told.
#[derive(Clone)]
pub enum ClientsMsg {
    /// The pool changed.
    Changed,
    /// Which clients use the pool (None: not read yet).
    Uses(Option<[bool; 2]>),
    /// What's running.
    Found(Box<Running>),
    /// The chart's span chosen.
    Span(usize),
    /// Restart pressed.
    Restart,
    /// Codex's service restarted (its new pid).
    Restarted(Result<i32, String>),
    /// Its chart steps on a minute at a time.
    Tick,
}

pub struct ClientsWindow {
    shared: Arc<Shared>,
    uses: Option<[bool; 2]>,
    span: usize,
    running: Option<Running>,
    restarting: bool,
    /// What the last restart came to (error?).
    note: Option<(String, bool)>,
    watching: Arc<AtomicBool>,
    scroll: ScrollHandle,
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
    /// The window's view; reads what's running until it closes.
    pub fn new(shared: Arc<Shared>, proxy_port: u16, uses: Option<[bool; 2]>, cx: &mut Cx<Self>) -> ClientsWindow {
        let watching = Arc::new(AtomicBool::new(true));
        let (w, found) = (watching.clone(), cx.messenger());
        let _ = std::thread::Builder::new().name("pool-clients".into()).spawn(move || {
            while w.load(Ordering::Relaxed) {
                found.send(ClientsMsg::Found(Box::new(olo_token_pool::running::scan(proxy_port))));
                std::thread::sleep(EVERY);
            }
        });
        cx.after(Duration::from_secs(30), ClientsMsg::Tick);
        ClientsWindow { shared, uses, span: 0, running: None, restarting: false, note: None, watching, scroll: ScrollHandle::new() }
    }

    /// The chart: requests a minute by client, with their totals below.
    fn activity(&self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let (name, span, step, grid) = SPANS[self.span];
        let pick = MenuField::new("pool-span", SPANS.iter().map(|s| s.0.into()).collect(), self.span).width(140.0).on_change(cx.msg_with(|i: &usize| ClientsMsg::Span(*i)));
        let t = now();
        let since = t - span * 60;
        let (clients, lines, totals) = self.shared.activity(|a| {
            let clients: Vec<String> = a.clients();
            let lines: Vec<VecDeque<(f64, f64)>> = clients.iter().map(|cl| a.series(cl, t, span, step)).collect();
            let totals: Vec<_> = clients.iter().map(|cl| a.totals(cl, since)).collect();
            (clients, lines, totals)
        });
        // Claude Code and Codex always; others once they've been seen.
        let shown: Vec<usize> = (0..clients.len()).filter(|&i| i < 2 || totals[i].requests > 0).collect();
        let series: Vec<Series> = shown.iter().map(|&i| Series::new(clients[i].clone(), style::c(p.hue(hue(&clients[i]))), Rc::new(lines[i].clone())).fill(true)).collect();
        let fmt = |v: f64| if v >= 10.0 || v == 0.0 { format!("{v:.0}/min") } else { format!("{v:.1}/min") };
        let graph = Graph::new(format!("pool-activity-{name}"), series, t as f64).span((span * 60) as f64).grid(grid).unit(fmt).smooth(false);
        let items: Vec<LegendItem> = shown
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
                LegendItem { color: style::c(p.hue(hue(&clients[i]))), name: clients[i].clone().into(), value: v.into() }
            })
            .collect();
        // As tall as its rows in the width it has (the window's content's:
        // more than one row only in a narrow window).
        let width = f32::from(cx.viewport_size().width) - 2.0 * m::WINDOW_INSET;
        let height = widgets::legend_height(width, &items);
        let look: Vec<(String, String, [u32; 4])> = items.iter().map(|it| (it.name.to_string(), it.value.to_string(), [it.color.h, it.color.s, it.color.l, it.color.a].map(f32::to_bits))).collect();
        let legend = canvas(move |g, b| widgets::legend(g, b, &items)).paint_key(style::key(look)).w_full().h(px(height));
        div()
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .child(div().flex().items_center().justify_between().child(heading("Activity")).child(pick))
            .child(div().w_full().h(px(GRAPH_HEIGHT)).child(graph))
            .child(legend)
    }

    /// Codex's service: what it is, and Restart.
    fn service(&self, should_use: bool, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let mut out = div().flex().flex_col().gap(px(m::TIGHT)).child(heading("Codex’s Service"));
        let t = now();
        let Some(running) = &self.running else {
            return out.child(StringView::new("Looking…").dim(true));
        };
        let Some(d) = &running.daemon else {
            return out.child(StringView::new("Not running: Codex starts it with its first window.").dim(true));
        };
        let version = d.version.as_deref().map(|v| format!("Codex {v}")).unwrap_or_else(|| "Codex".into());
        out = out.child(StringView::new(format!("{version} · since {} ({}) · pid {}", crate::clock(d.started), crate::span(t - d.started), d.pid)));
        let (route, wrong) = if d.stale {
            ("Started before the pool’s settings last changed, so still on the ones before (restarted onto them in a moment)".to_owned(), true)
        } else {
            let route = match (d.connections.pool, d.connections.direct) {
                (n, 0) if n > 0 => format!("through the pool ({n} connection{})", if n == 1 { "" } else { "s" }),
                _ => d.connections.describe(Provider::Codex),
            };
            (format!("On the current settings · {route}"), should_use && d.connections.direct > 0 && d.connections.pool == 0)
        };
        let color = if wrong { style::c(p.failure) } else { style::c(p.panel_text) };
        out = out.child(StringView::new(route).wrap(true).color(color));
        match &d.threads {
            Ok(threads) if threads.is_empty() => out = out.child(StringView::new("No conversations open.").dim(true)),
            Ok(threads) => {
                for th in threads {
                    let state = match (th.working, th.waiting) {
                        (true, true) => "waiting for you",
                        (true, false) => "working",
                        _ => "idle",
                    };
                    let title = if th.title.is_empty() { "A conversation".to_owned() } else { format!("“{}”", th.title) };
                    let place = th.cwd.as_deref().map(|d| format!(" in {}", folder(d))).unwrap_or_default();
                    out = out.child(StringView::new(format!("{title}: {state}{place}")).wrap(true));
                }
            }
            Err(e) => out = out.child(StringView::new(format!("Its conversations: it didn’t say ({e}).")).wrap(true).dim(true)),
        }
        let label = if self.restarting { "Restarting…" } else { "Restart" };
        let restart = Button::new(label).enabled(!self.restarting).on_click(cx.msg::<ClickEvent>(ClientsMsg::Restart));
        let said = match &self.note {
            Some((text, error)) => StringView::new(text.clone()).color(if *error { style::c(p.failure) } else { style::disabled_text(&p) }),
            None => StringView::new("Ends a turn going on; Codex’s windows reconnect.").dim(true),
        };
        out.child(div().pt(px(m::TIGHT)).flex().items_center().gap(px(m::SPACING)).child(restart).child(said))
    }

    /// Each Claude Code and Codex running, and where it connects.
    fn processes(&self, uses: Option<[bool; 2]>) -> Element {
        let p = theme::palette();
        let t = now();
        let mut out = div().flex().flex_col().gap(px(m::TIGHT)).child(heading("Running"));
        let Some(running) = &self.running else { return out };
        for (k, (prov, list)) in [(Provider::Claude, &running.claude), (Provider::Codex, &running.codex)].into_iter().enumerate() {
            let title = match list.len() {
                0 => format!("{}: none", prov.client()),
                n => format!("{} ({n})", prov.client()),
            };
            out = out.child(div().pt(px(m::TIGHT)).child(StringView::new(title).dim(list.is_empty())));
            let should_use = uses.is_some_and(|u| u[k]);
            for proc in list {
                let Process { pid, started, cwd, connections } = proc;
                let place = cwd.as_deref().map(folder).unwrap_or_else(|| "?".into());
                let line = format!("{} · since {} ({}) · pid {pid}", sentence(&connections.describe(prov)), crate::clock(*started), crate::span(t - started));
                // Codex's windows talk to its service; only Claude Code's
                // own connections say where its requests go (and some go
                // straight out on purpose: an MCP server).
                let wrong = should_use && connections.direct > 0 && connections.pool == 0 && prov == Provider::Claude;
                let color = if wrong { style::c(p.failure) } else { style::disabled_text(&p) };
                out = out.child(div().pt(px(2.0)).pl(px(m::SPACING)).flex().flex_col().child(StringView::new(place)).child(StringView::new(line).color(color)));
            }
        }
        out
    }
}

impl View for ClientsWindow {
    type Message = ClientsMsg;

    fn message(&mut self, message: ClientsMsg, cx: &mut Cx<Self>) {
        match message {
            ClientsMsg::Changed => {}
            ClientsMsg::Uses(uses) => self.uses = uses,
            ClientsMsg::Found(r) => self.running = Some(*r),
            ClientsMsg::Span(i) => self.span = i,
            ClientsMsg::Restart => {
                self.restarting = true;
                self.note = None;
                cx.work(|| codex_daemon::restart_now().map_err(|e| e.to_string()), ClientsMsg::Restarted);
            }
            ClientsMsg::Restarted(r) => {
                self.restarting = false;
                self.note = Some(match r {
                    Ok(pid) => (format!("Restarted (now pid {pid})."), false),
                    Err(e) => (e, true),
                });
            }
            ClientsMsg::Tick => cx.after(Duration::from_secs(30), ClientsMsg::Tick),
        }
        cx.invalidate();
    }

    fn command(&mut self, name: &str, cx: &mut Cx<Self>) -> bool {
        crate::ui::command(&self.shared, name, cx)
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        self.watching.store(false, Ordering::Relaxed);
        olo_kit::proxy().send(Msg::Closed(true));
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let uses = self.uses;
        let activity = self.activity(cx);
        let service = self.service(uses.is_some_and(|u| u[1]), cx);
        let processes = self.processes(uses);
        let content = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .p(px(m::WINDOW_INSET))
            .child(activity)
            .child(separator())
            .child(div().flex_1().min_h_0().child(ScrollView::new(&self.scroll).border(false).child(div().flex().flex_col().gap(px(m::SPACING)).child(service).child(processes))));
        div().size_full().flex().flex_col().child(crate::ui::menu_bar(cx)).child(content)
    }
}
