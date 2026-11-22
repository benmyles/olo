//! Rex's windows besides the workspace:
//! - Hosts: the hosts Rex knows, how each stands, and the ports
//!   forwarded from them;
//! - Remote Access: this computer's own Rex server reached over
//!   Tailscale, and a device paired with it by a code to scan;
//! - Diagnostics: the facts of the server and of this app, to copy;
//! - the question an SSH connection asks (a password, a host key to
//!   trust).
//!
//! Rex has no settings window of its own. What it does for whom is
//! Preferences ▸ Rex (the desktop's settings: `shared`); its keys are
//! Preferences ▸ Keyboard; a window's quick choices are its Settings
//! menu. (It came with a window of eight sections, most of whose
//! controls had nothing behind them.)

use std::sync::Arc;

use olo_kit::{
    Bounds, Child, Element, IntoElement as _, ParentElement as _, PathBuilder, Pixels, Point, SharedString, Styled as _, WindowCx, canvas, div, metrics as m, point, px, size, style, theme,
    widgets::{BBox, Button, CheckBox, MenuField, StringView, TextControl, TextField, divider_for},
};
use olo_rex_client::{AgentStatus, SshAskpass};
use olo_sync::{Condvar, Mutex};
use zeroize::Zeroizing;

use crate::hub::{HostConnState, RexHub};

/// A status light's colours, in every window of Rex's: on, waiting
/// (connecting, to be signed in), down, and off (nothing wrong: not
/// in use).
const LIGHT_ON: [u8; 4] = [56, 174, 92, 255];
const LIGHT_WAITING: [u8; 4] = [232, 168, 36, 255];
const LIGHT_DOWN: [u8; 4] = [210, 64, 64, 255];
const LIGHT_OFF: [u8; 4] = [140, 140, 148, 160];

/// A status light before a line of text: 8 points across and no wider
/// than it looks, so that its edge is the edge of what it is set under
/// (in a box of 10 it stood a point to the right of the label above
/// it); to be centred on the line beside it.
fn light(color: [u8; 4]) -> Element {
    canvas(move |g, b| {
        paint_dot(g, b.center(), 4.0, style::c(color));
    })
    .paint_key(style::key(color))
    .w(px(8.0))
    .h(px(8.0))
    .flex_shrink_0()
    .into_any_element()
}

fn paint_dot(g: &mut olo_kit::Painter, center: Point<Pixels>, radius: f32, color: olo_kit::Hsla) {
    let mut pb = PathBuilder::fill();
    let cx = f32::from(center.x);
    let cy = f32::from(center.y);
    let steps = 16;
    for i in 0..steps {
        let a = (i as f32 / steps as f32) * std::f32::consts::TAU;
        let pt = point(px(cx + radius * a.cos()), px(cy + radius * a.sin()));
        if i == 0 { pb.move_to(pt) } else { pb.line_to(pt) }
    }
    pb.close();
    if let Ok(path) = pb.build() {
        g.paint_path(path, color);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_hosts_panel(
    cx: &WindowCx,
    hub: &Arc<RexHub>,
    waits_for_agent: &dyn Fn(&str) -> bool,
    host_label_field: &Child<TextField>,
    host_addr_field: &Child<TextField>,
    fwd_port_field: &Child<TextField>,
    fwd_host: Option<&str>,
    on_add_host: impl Fn(&(), &mut WindowCx) + 'static,
    on_reconnect_host: impl Fn(&String, &mut WindowCx) + 'static,
    on_remove_host: impl Fn(&String, &mut WindowCx) + 'static,
    on_forward_port: impl Fn(&(String, String, u16, bool), &mut WindowCx) + 'static,
    on_stop_forward: impl Fn(&String, &mut WindowCx) + 'static,
    on_add_manual_forward: impl Fn(&(), &mut WindowCx) + 'static,
    on_fwd_host: impl Fn(&String, &mut WindowCx) + 'static,
    on_close: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    let on_recon = std::rc::Rc::new(on_reconnect_host);
    let on_rem = std::rc::Rc::new(on_remove_host);
    let on_fwd = std::rc::Rc::new(on_forward_port);
    let on_stop = std::rc::Rc::new(on_stop_forward);

    let can_add = !host_addr_field.read(cx).text().trim().is_empty();
    let port_typed = fwd_port_field.read(cx).text().trim().parse::<u16>().is_ok_and(|p| p > 0);

    let (hosts, detected_ports, active_forwards, with_sessions) = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let with_sessions: std::collections::HashSet<String> = st.hosts.values().filter(|h| !h.session_summaries.is_empty()).map(|h| h.key.clone()).collect();
        let hosts: Vec<(String, String, String, bool, HostConnState)> = st
            .host_order
            .iter()
            .filter_map(|k| {
                let h = st.hosts.get(k)?;
                Some((h.key.clone(), h.label.clone(), h.endpoint_raw.clone(), h.is_local, h.conn_state.clone()))
            })
            .collect();
        let mut ports = Vec::new();
        for (bk, br) in &st.blocks {
            let g = br.lock().unwrap_or_else(|e| e.into_inner());
            let host_lbl = st.hosts.get(&bk.host_key).map(|h| h.label.clone()).unwrap_or_else(|| bk.host_key.clone());
            let proc_name = g.process.as_ref().and_then(|p| p.foreground.as_ref().or(p.child.as_ref())).map(|p| p.name.clone());
            for pe in &g.ports.ports {
                let here = st.hosts.get(&bk.host_key).is_some_and(|h| h.is_local);
                ports.push((bk.host_key.clone(), host_lbl.clone(), bk.session_id.clone(), pe.address.clone(), pe.port, proc_name.clone(), here));
            }
        }
        let forwards: Vec<_> = st
            .forwards
            .iter()
            .map(|f| {
                let lbl = st.hosts.get(&f.host_key).map(|h| h.label.clone()).unwrap_or_else(|| f.host_key.clone());
                (f.id.clone(), lbl, f.remote_host.clone(), f.remote_port, f.local_addr.port(), f.state.clone())
            })
            .collect();
        (hosts, ports, forwards, with_sessions)
    };

    // The hosts a port can be forwarded from: the others' (a port of
    // this computer is here already), the one chosen among them, and
    // whether it has a session to forward through. (The menu listed
    // every host and chose nothing: the port was forwarded from
    // whichever host the app came upon first.)
    let from: Vec<(String, SharedString)> = hosts.iter().filter(|(_, _, _, local, _)| !local).map(|(key, label, ..)| (key.clone(), label.clone().into())).collect();
    let chosen = fwd_host.and_then(|key| from.iter().position(|(k, _)| k == key)).unwrap_or(0);
    let can_fwd = port_typed && from.get(chosen).is_some_and(|(key, _)| with_sessions.contains(key));
    let from_keys: Vec<String> = from.iter().map(|(key, _)| key.clone()).collect();
    let from_names: Vec<SharedString> = from.iter().map(|(_, name)| name.clone()).collect();

    let mut host_list = div().flex().flex_col().gap(px(m::SPACING)).w_full();
    for (key, label, ep, is_local, conn) in hosts {
        let conn_dot: [u8; 4] = match &conn {
            HostConnState::Connected => LIGHT_ON,
            HostConnState::Connecting | HostConnState::Reconnecting { .. } => LIGHT_WAITING,
            HostConnState::Disconnected | HostConnState::Error(_) => LIGHT_DOWN,
        };
        let detail = match &conn {
            HostConnState::Connected => ep,
            HostConnState::Connecting if waits_for_agent(&ep) => olo_kit::tr!("{}  •  Waiting for your key agent to sign…", ep),
            HostConnState::Connecting => format!("{ep}  •  Connecting…"),
            HostConnState::Disconnected => format!("{ep}  •  Disconnected"),
            HostConnState::Reconnecting { attempt, .. } => format!("{ep}  •  Reconnecting (#{attempt})…"),
            HostConnState::Error(e) => format!("{ep}  •  {e}"),
        };
        let r_cb = on_recon.clone();
        let d_cb = on_rem.clone();
        let k1 = key.clone();
        let k2 = key;
        // Always place `Remove` to the left of `Reconnect`, with `Reconnect` pinned in the rightmost column
        // so `Local`'s `Reconnect` aligns directly above every remote host's `Reconnect` (never over `Remove`)!
        let mut row = div().flex().flex_row().items_center().gap(px(m::SPACING)).child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().flex().flex_row().items_center().gap(px(6.0)).child(light(conn_dot)).child(StringView::new(label).bold(true).cut(true)))
                .child(div().pl(px(16.0)).child(StringView::new(detail).dim(true).cut(true))),
        );
        if !is_local {
            row = row.child(Button::new("Remove").on_click(move |_, cx| d_cb(&k2, cx)));
        }
        row = row.child(Button::new("Reconnect").on_click(move |_, cx| r_cb(&k1, cx)));
        host_list = host_list.child(row);
    }

    let mut port_list = div().flex().flex_col().gap(px(m::SPACING)).w_full();
    if detected_ports.is_empty() && active_forwards.is_empty() {
        port_list = port_list.child(StringView::new("No listening ports or active forwards.").dim(true));
    }
    for (hk, host_lbl, sid, addr, port, proc_name, here) in detected_ports.into_iter().take(8) {
        let cb = on_fwd.clone();
        let (hk_c, sid_c) = (hk.clone(), sid.clone());
        let endpoint_str = match proc_name {
            Some(p) if !p.is_empty() => format!("{addr}:{port} ({p})"),
            _ => format!("{addr}:{port}"),
        };
        port_list = port_list.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(m::SPACING))
                .child(div().flex_1().min_w_0().flex().flex_col().child(StringView::new(endpoint_str).bold(true).cut(true)).child(StringView::new(host_lbl).dim(true).cut(true)))
                // (A port of this computer is here already: it is opened. Another host's is forwarded here first.)
                .child(Button::new(if here { "Open" } else { "Forward and Open" }).on_click(move |_, cx| {
                    cb(&(hk_c.clone(), sid_c.clone(), port, !here), cx);
                })),
        );
    }
    for (fid, host_lbl, rhost, rport, lport, state) in active_forwards {
        let stop = on_stop.clone();
        let fid_c = fid.clone();
        // (A forward that couldn't be restored isn't running: its row says
        // so in its ink, and what is offered is to remove it, not to stop
        // it.)
        let failed = matches!(state, crate::hub::ForwardState::Failed(_));
        let status = match state {
            crate::hub::ForwardState::Active => None,
            crate::hub::ForwardState::Waiting => Some(StringView::new("Waiting to reconnect.").dim(true)),
            crate::hub::ForwardState::Failed(why) => Some(StringView::new(olo_kit::tr!("Couldn’t restore this forward: {}", why)).color(style::c(theme::palette().failure)).wrap(true)),
        };
        port_list = port_list.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(m::SPACING))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(StringView::new(format!("127.0.0.1:{lport}  →  {host_lbl} ({rhost}:{rport})")).bold(!failed).dim(failed).cut(true))
                        .children(status),
                )
                .child(Button::new(if failed { "Remove" } else { "Stop" }).on_click(move |_, cx| stop(&fid_c, cx))),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap(px(m::SECTION))
        .p(px(m::WINDOW_INSET))
        .w_full()
        .child(BBox::new("Hosts").child(host_list))
        .child(
            BBox::new("Add Host").child(
                div()
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap(px(m::SPACING))
                    .child(div().w(px(132.0)).flex_shrink_0().flex().flex_col().gap(px(3.0)).child(StringView::new("Label").dim(true)).child(TextControl::new(cx, host_label_field)))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.0)).child(StringView::new("Address").dim(true)).child(TextControl::new(cx, host_addr_field)))
                    .child(Button::new("Add and Connect").enabled(can_add).on_click(move |_, cx| on_add_host(&(), cx))),
            ),
        )
        .child(
            BBox::new("Ports").child(
                div().flex().flex_col().gap(px(m::SPACING)).child(port_list).child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(m::SPACING))
                        .child(div().w(px(84.0)).flex_shrink_0().child(TextControl::new(cx, fwd_port_field)))
                        .child(div().flex_1().min_w_0().child(MenuField::new("fwd-host", from_names, chosen).label("From:").width(180.0).enabled(from_keys.len() > 1).on_change(move |i, cx| {
                            if let Some(key) = from_keys.get(*i) {
                                on_fwd_host(key, cx);
                            }
                        })))
                        .child(Button::new("Forward Port").enabled(can_fwd).on_click(move |_, cx| on_add_manual_forward(&(), cx))),
                ),
            ),
        )
        .child(div().w_full().flex().flex_row().justify_end().child(Button::new("Close").min_width(m::BUTTON_MIN_WIDTH).default(true).on_click(move |_, cx| on_close(&(), cx))))
}

fn qr_code(url: &str, qr: Arc<qrcode::QrCode>, side: f32) -> Element {
    canvas(move |g, b| {
        let p = theme::palette();
        style::fill_rect(g, b, style::c([255, 255, 255, 255]));
        style::sunken(g, b, &p, false);

        let w = qr.width();
        let avail = f32::from(b.size.width).min(f32::from(b.size.height)) - 4.0;
        let scale = g.scale_factor();
        let cell = ((avail / (w + 8) as f32) * scale).floor().max(1.0) / scale;
        let total = cell * w as f32;
        let ox = f32::from(b.left()) + ((f32::from(b.size.width) - total) / 2.0 * scale).round() / scale;
        let oy = f32::from(b.top()) + ((f32::from(b.size.height) - total) / 2.0 * scale).round() / scale;
        let dark = style::c([0, 0, 0, 255]);
        for y in 0..w {
            for x in 0..w {
                if qr[(x, y)] == qrcode::Color::Dark {
                    g.paint_quad(olo_kit::fill(Bounds::new(point(px(ox + x as f32 * cell), px(oy + y as f32 * cell)), size(px(cell), px(cell))), dark));
                }
            }
        }
    })
    .paint_key(style::key(url))
    .w(px(side))
    .h(px(side))
}

/// The side of the code to scan, in points.
const CODE: f32 = 144.0;

/// Remote Access: this computer's Rex server reached over Tailscale,
/// and what that takes at each step, in one group that keeps its place
/// and size whatever the state: a sign-in (the page opened here, or its
/// code scanned), then a device paired (the server's address as a code
/// for the phone, or copied for another computer). It was two places:
/// a section of the Settings window, and a window with the code alone
/// that said to go and look for the other.
pub fn render_access_panel(
    hub: &Arc<RexHub>,
    feedback: Option<Element>,
    code: Option<Arc<qrcode::QrCode>>,
    on_tailscale: impl Fn(&crate::tailscale::Action, &mut WindowCx) + 'static,
    on_copy: impl Fn(&String, &mut WindowCx) + 'static,
    on_close: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    use crate::tailscale::Action;
    let (status, connected) = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let h = st.hosts.get("local");
        (h.and_then(|x| x.tailscale.clone()), h.is_some_and(|x| x.conn_state == HostConnState::Connected))
    };
    let status = status.as_ref().filter(|_| connected);
    let step = Step::of(status);
    let on = status.is_some_and(|s| matches!(s.state.as_str(), "starting" | "needs_login" | "running" | "connected" | "needs_machine_auth" | "stopping"));
    let on_action = std::rc::Rc::new(on_tailscale);
    let toggle = on_action.clone();
    let check = CheckBox::new("Let your other devices reach this computer’s sessions over Tailscale", on)
        .enabled(status.is_some())
        .on_toggle(move |on, cx| toggle(&if *on { Action::Enable } else { Action::Disable }, cx));
    // Under the check, in line with its words: how it stands, or what
    // just went wrong with it. One line's room always, so nothing below
    // moves as it comes and goes.
    let (lit, stands): ([u8; 4], String) = match &step {
        // (The address is in the group below: not said twice.)
        Step::Pair(_) => (LIGHT_ON, olo_kit::words::tr("Your other devices on Tailscale can reach these sessions.").into()),
        Step::SignIn(_) => (LIGHT_WAITING, olo_kit::words::tr("Waiting for you to sign in…").into()),
        Step::Off if !connected => (LIGHT_DOWN, crate::tailscale::state_line(status, connected)),
        Step::Off if status.is_some_and(|s| matches!(s.state.as_str(), "starting" | "running" | "connected" | "stopping")) => (LIGHT_WAITING, crate::tailscale::state_line(status, connected)),
        Step::Off => (LIGHT_OFF, crate::tailscale::state_line(status, connected)),
    };
    // (What just went wrong takes the line's place, without a light: it has its own colour.)
    let line = match feedback {
        Some(problem) => problem,
        None => div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::MARK_GAP))
            .child(light(lit))
            .child(div().flex_1().min_w_0().child(StringView::new(stands).dim(true).wrap(true)))
            .into_any_element(),
    };
    let stands = div().w_full().pl(px(m::MARK_SIZE + m::MARK_GAP)).min_h(px(olo_kit::widgets::TextView::line_height())).child(line);

    let (title, says, address, button): (&str, &str, Option<String>, Option<Element>) = match &step {
        Step::Off => ("Pair a Device", "Turn on remote access, then scan the code with Rex on your phone.", None, None),
        Step::SignIn(link) => {
            let open = on_action.clone();
            let button = match link {
                Some(page) => {
                    let page = page.clone();
                    Button::new("Open Sign-In Page").on_click(move |_, cx| open(&Action::OpenLink(page.clone()), cx))
                }
                None => Button::new("Sign In to Tailscale…").on_click(move |_, cx| open(&Action::Login, cx)),
            };
            (
                "Sign In to Tailscale",
                "Open the sign-in page on this computer, or scan the code with your phone. After you sign in, this window shows the pairing code.",
                None,
                Some(button.into_any_element()),
            )
        }
        Step::Pair(url) => {
            let copy = url.clone();
            (
                "Pair a Device",
                "Scan the code with Rex on your phone, or use this address in Rex on another computer:",
                Some(url.clone()),
                Some(Button::new("Copy Address").on_click(move |_, cx| on_copy(&copy, cx)).into_any_element()),
            )
        }
    };
    let shown = step.code().map(str::to_owned);
    let well = match (shown, code) {
        (Some(url), Some(code)) => qr_code(&url, code, CODE),
        // No code to show (or it is still being made): its place, with a
        // code's three corner marks faintly, so that it reads as where
        // one will be and not as something that failed to draw.
        _ => canvas(|g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, style::shade(&p, p.panel_background, 1.04));
            style::sunken(g, b, &p, false);
            let scale = g.scale_factor();
            // 25 cells a side with 4 of quiet around, each a whole number of device pixels.
            let cell = ((CODE - 4.0) / 33.0 * scale).floor().max(1.0) / scale;
            let (ox, oy) = (f32::from(b.left()) + ((CODE - cell * 25.0) / 2.0 * scale).round() / scale, f32::from(b.top()) + ((CODE - cell * 25.0) / 2.0 * scale).round() / scale);
            let ink = style::c(p.panel_text).opacity(0.16);
            let ground = style::shade(&p, p.panel_background, 1.04);
            let square = |g: &mut olo_kit::Painter, x: f32, y: f32, cells: f32, color| {
                g.paint_quad(olo_kit::fill(Bounds::new(point(px(ox + x * cell), px(oy + y * cell)), size(px(cells * cell), px(cells * cell))), color))
            };
            for (x, y) in [(0.0, 0.0), (18.0, 0.0), (0.0, 18.0)] {
                square(g, x, y, 7.0, ink);
                square(g, x + 1.0, y + 1.0, 5.0, ground);
                square(g, x + 2.0, y + 2.0, 3.0, ink);
            }
        })
        .paint_key(style::key("no-code"))
        .w(px(CODE))
        .h(px(CODE)),
    };
    // Beside the code, from its top: what to do, the address, the button.
    let beside = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .items_start()
        .gap(px(m::SPACING))
        .child(StringView::new(says).wrap(true))
        // (The address is a value, not more of the sentence: bold, on a
        // line the height of the text's own, so the column keeps one
        // rhythm in every state.)
        .children(address.map(|a| div().w_full().child(StringView::new(a).bold(true).cut(true).height(m::LABEL_ROW))))
        .children(button);
    let group = div().w_full().flex().flex_row().items_start().gap(px(m::SECTION)).child(div().flex_shrink_0().child(well)).child(beside);

    div()
        .flex()
        .flex_col()
        .gap(px(m::SECTION))
        .p(px(m::WINDOW_INSET))
        .w_full()
        .child(div().w_full().flex().flex_col().gap(px(m::TIGHT)).child(check).child(stands))
        .child(BBox::new(title).child(group))
        .child(div().w_full().flex().flex_row().justify_end().child(Button::new("Close").min_width(m::BUTTON_MIN_WIDTH).default(true).on_click(move |_, cx| on_close(&(), cx))))
}

/// Where reaching this server from elsewhere stands, as the Remote
/// Access window's group shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Tailscale off, starting, or the server not connected: nothing to
    /// scan yet.
    Off,
    /// The server is to be signed in to Tailscale: its page, once the
    /// server has given the link.
    SignIn(Option<String>),
    /// On the tailnet: the address a device pairs with.
    Pair(String),
}

impl Step {
    pub fn of(status: Option<&olo_rex_client::TailscaleStatus>) -> Step {
        let Some(s) = status else { return Step::Off };
        match s.state.as_str() {
            "needs_login" => Step::SignIn(crate::tailscale::login_link(s).map(str::to_owned)),
            "running" | "connected" => s
                .https
                .as_ref()
                .and_then(|h| h.url.clone())
                .or_else(|| s.dns_name.as_ref().filter(|d| !d.is_empty()).map(|d| format!("https://{}", d.trim_end_matches('.'))))
                .map_or(Step::Off, Step::Pair),
            _ => Step::Off,
        }
    }

    /// What the code to scan says, if there is one to show.
    pub fn code(&self) -> Option<&str> {
        match self {
            Step::SignIn(link) => link.as_deref(),
            Step::Pair(url) => Some(url),
            Step::Off => None,
        }
    }
}

fn build_info(version: &olo_rex_client::ServerVersion) -> String {
    let mut parts = Vec::new();
    if !version.platform.is_empty() {
        parts.push(version.platform.as_str());
    }
    if !version.build_date.is_empty() {
        parts.push(version.build_date.as_str());
    }
    if version.dirty {
        parts.push("modified source");
    }
    parts.join(" · ")
}

/// How the SSH agent stands, in a line (Diagnostics).
fn agent_line(agent: &AgentStatus) -> String {
    if agent.reachable {
        olo_kit::tr!("{} keys, at {}", agent.key_count, agent.socket_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default())
    } else {
        // (With no socket at all, the client's own words only say the label again.)
        agent.error.clone().filter(|_| agent.socket_path.is_some()).unwrap_or_else(|| olo_kit::words::tr("None found").into())
    }
}

pub fn render_diagnostics_panel(
    hub: &Arc<RexHub>,
    agent: &AgentStatus,
    on_copy_version: impl Fn(&String, &mut WindowCx) + 'static,
    on_copy_debug: impl Fn(&String, &mut WindowCx) + 'static,
    on_close: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    let (blocks_n, res_bytes, img_bytes) = hub.total_vt_memory();
    let (version_info, debug_info, rows): (String, String, Vec<(&'static str, String)>) = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let local = st.hosts.get("local");
        let ver = local.and_then(|h| h.server_version.as_ref()).map(|v| format!("{} (commit {}, API rev {})", v.version, v.commit, v.api_revision)).unwrap_or_else(|| "Disconnected".to_owned());
        let status = local.and_then(|h| h.server_status.as_ref());
        let pid_val = status.map(|s| format!("PID {}", s.pid)).unwrap_or_else(|| "—".to_owned());
        let inst_val = status.map(|s| s.instance_id.clone()).unwrap_or_else(|| "—".to_owned());
        let sess_val = status.map(|s| format!("{} on this server", s.session_count)).unwrap_or_else(|| "0 on this server".to_owned());
        let hosts_val = format!("{} configured", st.hosts.len());
        let listeners = status.and_then(|s| s.listeners.as_ref()).map(|ls| ls.iter().map(|l| l.endpoint.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_else(|| "None".to_owned());
        let vt_val = format!("libghostty-vt ({blocks_n} live panes)");
        let mem_val = format!("{:.1} MiB of resident grid pages", res_bytes as f64 / (1024.0 * 1024.0));
        let img_val = if img_bytes == 0 { "None".to_owned() } else { format!("{:.1} MiB", img_bytes as f64 / (1024.0 * 1024.0)) };
        let mut ver_copy = format!("Olo Rex {}\nRex server {ver}", env!("CARGO_PKG_VERSION"));
        if let Some(version) = local.and_then(|h| h.server_version.as_ref()) {
            let build = build_info(version);
            if !build.is_empty() {
                ver_copy.push('\n');
                ver_copy.push_str(&build);
            }
        }
        let agent_val = agent_line(agent);
        let dbg_copy = format!(
            "{ver_copy}\nProcess: {pid_val}\nInstance: {inst_val}\nSessions: {sess_val}\nHosts: {hosts_val}\nListeners: {listeners}\nSSH agent: {agent_val}\nVT engine: {vt_val}\nMemory: {mem_val}\nKitty images: {img_val}"
        );
        (
            ver_copy,
            dbg_copy,
            vec![
                ("Server:", ver),
                ("Process:", pid_val),
                ("Instance:", inst_val),
                ("Sessions:", sess_val),
                ("Hosts:", hosts_val),
                ("Listeners:", listeners),
                ("SSH agent:", agent_val),
                ("VT engine:", vt_val),
                ("Memory:", mem_val),
                ("Kitty images:", img_val),
            ],
        )
    };

    let label_col_w = (divider_for(&["Server:", "Process:", "Instance:", "Sessions:", "Hosts:", "Listeners:", "SSH agent:", "VT engine:", "Memory:", "Kitty images:"]) - m::LABEL_GAP).max(64.0);
    let mut grid = div().flex().flex_col().gap(px(4.0)).w_full();
    for (label, value) in rows {
        grid = grid.child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().w(px(label_col_w)).flex_shrink_0().flex().flex_row().justify_end().child(StringView::new(label).dim(true)))
                .child(div().flex_1().min_w_0().child(StringView::new(value).cut(true))),
        );
    }

    div().flex().flex_col().gap(px(m::WINDOW_INSET)).p(px(m::WINDOW_INSET)).w_full().child(grid).child(
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .child(Button::new("Copy Version Info").on_click(move |_, cx| on_copy_version(&version_info, cx)))
                    .child(Button::new("Copy Debug Info").on_click(move |_, cx| on_copy_debug(&debug_info, cx))),
            )
            .child(Button::new("Close").min_width(m::BUTTON_MIN_WIDTH).default(true).on_click(move |_, cx| on_close(&(), cx))),
    )
}

#[derive(Debug, Clone)]
pub enum AskpassRequest {
    ConfirmHostKey { host: String, port: u16, fingerprint: String, key_type: String, changed: bool },
    Secret { title: String, prompt: String, echo: bool },
}

#[derive(Debug, Clone)]
pub enum AskpassReply {
    Confirm(bool),
    Secret(Option<String>),
}

pub struct OloAskpass {
    /// One question at a time: whoever asks holds this until answered.
    turn: Mutex<()>,
    pending: Mutex<Option<(AskpassRequest, Option<AskpassReply>)>>,
    cv: Condvar,
    on_request: Arc<dyn Fn() + Send + Sync>,
    /// The hosts whose connection waits for the key agent to sign
    /// (`waiting_for_agent`), and what to do when that changes.
    agent_waits: Mutex<Vec<String>>,
    on_change: Arc<dyn Fn() + Send + Sync>,
}

impl std::fmt::Debug for OloAskpass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OloAskpass").finish_non_exhaustive()
    }
}

impl OloAskpass {
    /// `on_request`: a question waits to be shown. `on_change`: which
    /// hosts wait for the key agent changed.
    pub fn new(on_request: impl Fn() + Send + Sync + 'static, on_change: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self { turn: Mutex::new(()), pending: Mutex::new(None), cv: Condvar::new(), on_request: Arc::new(on_request), agent_waits: Mutex::new(Vec::new()), on_change: Arc::new(on_change) })
    }

    /// Whether the connection to `host` (a name or an address, as its
    /// endpoint has it) is waiting for the key agent to sign.
    pub fn waits_for_agent(&self, endpoint: &str) -> bool {
        self.agent_waits.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|host| endpoint.contains(host.as_str()))
    }

    pub fn current_request(&self) -> Option<AskpassRequest> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(_, rep)| rep.is_none()).map(|(req, _)| req.clone())
    }

    pub fn answer(&self, reply: AskpassReply) {
        let mut g = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, slot)) = g.as_mut() {
            *slot = Some(reply);
            self.cv.notify_all();
        }
    }

    /// One question, and its own answer. Two hosts connecting at once
    /// each ask about their key: the second waits for the first to be
    /// answered. (There was one slot and no turn: the second question
    /// replaced the first, and the answer to it went to whichever of
    /// the two woke first, so "Trust" said of one host's key could
    /// trust the other's, which the person never saw.)
    fn ask(&self, req: AskpassRequest) -> AskpassReply {
        let _turn = self.turn.lock().unwrap_or_else(|e| e.into_inner());
        let mut g = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some((req, None));
        drop(g);
        (self.on_request)();
        let mut g = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some((_, Some(rep))) = g.as_ref() {
                let out = rep.clone();
                *g = None;
                return out;
            }
            g = self.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl SshAskpass for OloAskpass {
    fn confirm_host_key(&self, host: &str, port: u16, fingerprint: &str, key_type: &str, changed: bool) -> bool {
        match self.ask(AskpassRequest::ConfirmHostKey { host: host.to_owned(), port, fingerprint: fingerprint.to_owned(), key_type: key_type.to_owned(), changed }) {
            AskpassReply::Confirm(ok) => ok,
            _ => false,
        }
    }

    fn prompt_secret(&self, title: &str, prompt: &str, echo: bool) -> Option<Zeroizing<String>> {
        match self.ask(AskpassRequest::Secret { title: title.to_owned(), prompt: prompt.to_owned(), echo }) {
            AskpassReply::Secret(Some(s)) => Some(Zeroizing::new(s)),
            _ => None,
        }
    }

    fn waiting_for_agent(&self, host: &str, waiting: bool) {
        {
            let mut waits = self.agent_waits.lock().unwrap_or_else(|e| e.into_inner());
            waits.retain(|h| h != host);
            if waiting {
                waits.push(host.to_owned());
            }
        }
        (self.on_change)();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn diagnostics_use_the_real_build_fields() {
        let v = serde_json::from_value(serde_json::json!({"rex_version":"1.2","platform":"linux_amd64","built_at":"fixture-date","dirty":true})).unwrap();
        assert_eq!(super::build_info(&v), "linux_amd64 · fixture-date · modified source");
    }

    use olo_rex_client::{TailscaleHttps, TailscaleStatus};

    use super::Step;

    /// Two hosts ask about their keys at once: each question is shown
    /// in turn and gets the answer given to it, never the other's.
    #[test]
    fn each_question_gets_its_own_answer() {
        use super::{AskpassReply, AskpassRequest, OloAskpass};
        use olo_rex_client::SshAskpass as _;
        let ui = OloAskpass::new(|| {}, || {});
        let ask = |host: &'static str| {
            let ui = ui.clone();
            std::thread::spawn(move || ui.confirm_host_key(host, 22, "SHA256:fixture", "ssh-ed25519", false))
        };
        let (a, b) = (ask("trusted.example"), ask("refused.example"));
        let mut answered = Vec::new();
        for _ in 0..2 {
            // Wait for a question to be shown, then answer by what it asks.
            let host = loop {
                match ui.current_request() {
                    Some(AskpassRequest::ConfirmHostKey { host, .. }) if !answered.contains(&host) => break host,
                    _ => std::thread::sleep(std::time::Duration::from_millis(5)),
                }
            };
            ui.answer(AskpassReply::Confirm(host == "trusted.example"));
            answered.push(host);
        }
        assert!(a.join().unwrap(), "the host the person trusted");
        assert!(!b.join().unwrap(), "the host the person refused is not trusted by the other's answer");
        // The key agent's waits, by host.
        ui.waiting_for_agent("build.example", true);
        assert!(ui.waits_for_agent("ssh://dev@build.example") && !ui.waits_for_agent("ssh://dev@other.example"));
        ui.waiting_for_agent("build.example", false);
        assert!(!ui.waits_for_agent("ssh://dev@build.example"));
    }

    /// What the Remote Access window offers at each state of the
    /// server's Tailscale: nothing to scan while it is off, starting or
    /// unknown; the sign-in page once the server has given its link
    /// (and a way to ask for it before); the address to pair with once
    /// it is on, the HTTPS one where the server has it.
    #[test]
    fn the_step_follows_the_servers_tailscale() {
        let state = |state: &str| TailscaleStatus { state: state.into(), ..Default::default() };
        assert_eq!(Step::of(None), Step::Off);
        for off in ["disabled", "stopped", "starting", "stopping", "something-new"] {
            assert_eq!(Step::of(Some(&state(off))), Step::Off, "{off}");
        }
        assert_eq!(Step::of(Some(&state("needs_login"))), Step::SignIn(None));
        let asked = TailscaleStatus { auth_url: Some("https://login.example/a/1".into()), ..state("needs_login") };
        assert_eq!(Step::of(Some(&asked)), Step::SignIn(Some("https://login.example/a/1".into())));
        assert_eq!(Step::of(Some(&asked)).code(), Some("https://login.example/a/1"));
        // (A link that isn't one is not offered, nor drawn as a code.)
        let odd = TailscaleStatus { auth_url: Some("javascript:alert(1)".into()), ..state("needs_login") };
        assert_eq!(Step::of(Some(&odd)), Step::SignIn(None));
        let named = TailscaleStatus { dns_name: Some("studio.example.ts.net.".into()), ..state("running") };
        assert_eq!(Step::of(Some(&named)), Step::Pair("https://studio.example.ts.net".into()));
        let served = TailscaleStatus { https: Some(TailscaleHttps { url: Some("https://studio.example.ts.net:8443".into()), ..Default::default() }), ..named.clone() };
        assert_eq!(Step::of(Some(&served)).code(), Some("https://studio.example.ts.net:8443"));
        // On, but the server hasn't said where yet: nothing to scan.
        assert_eq!(Step::of(Some(&state("running"))), Step::Off);
        assert_eq!(Step::Off.code(), None);
    }
}
