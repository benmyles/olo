//! Olo Rex: native Olo desktop client for Rex ("the multiplexer for all work").
//!
//! Built on `olo-rex-vt` (`libghostty-vt`), `olo-rex-client` (Rex API rev 19 +
//! native Rust SSH + tunnels), and `olo-kit` (120 Hz GPU rendering, BeOS/Olo chrome,
//! in-place upgrade survival, Deskbar Glance, and zero-trust Olo Desktop Bridge).

mod account;
mod account_panel;
mod activity_watch;
mod agent;
mod attach;
mod attention;
mod bridge;
mod bridge_run;
mod carry;
mod carry_panel;
mod copy_mode;
mod files_sock;
mod forwards;
mod frame;
mod git;
mod host;
mod host_panel;
mod host_screens;
mod hub;
mod keymap;
mod link;
mod links;
mod moving;
mod overlays;
mod panels;
mod pool;
mod predict;
mod repos;
mod server;
mod server_panel;
mod settings;
mod setup;
mod shared;
mod subagents;
mod tailscale;
mod terminal_pane;
mod themes;
mod transcript_view;
mod ui;

use olo_kit::instance::{self, Launch};

use crate::{
    settings::Settings,
    ui::{APP_ID, SavedAppState, SavedWorkspaceState},
};

struct CliArgs {
    new_window: bool,
    open_settings: bool,
    open_hosts: bool,
    open_diagnostics: bool,
    open_qr: bool,
    dir: Option<String>,
    session: Option<(String, String)>,
    focus_host: Option<String>,
    focus_block: Option<(String, String, String)>,
}

fn parse_args(args: &[String]) -> CliArgs {
    let mut out = CliArgs { new_window: false, open_settings: false, open_hosts: false, open_diagnostics: false, open_qr: false, dir: None, session: None, focus_host: None, focus_block: None };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--new-window" => out.new_window = true,
            "--settings" => out.open_settings = true,
            "--hosts" => out.open_hosts = true,
            "--diagnostics" => out.open_diagnostics = true,
            "--qr" => out.open_qr = true,
            "--dir" | "-d" => {
                if let Some(v) = args.get(i + 1) {
                    out.dir = Some(v.clone());
                    i += 1;
                }
            }
            "--session" => {
                if let (Some(h), Some(s)) = (args.get(i + 1), args.get(i + 2)) {
                    out.session = Some((h.clone(), s.clone()));
                    i += 2;
                }
            }
            "--focus-host" => {
                if let Some(h) = args.get(i + 1) {
                    out.focus_host = Some(h.clone());
                    i += 1;
                }
            }
            "--focus-block" => {
                if let (Some(h), Some(s), Some(b)) = (args.get(i + 1), args.get(i + 2), args.get(i + 3)) {
                    out.focus_block = Some((h.clone(), s.clone(), b.clone()));
                    i += 3;
                }
            }
            other if !other.starts_with('-') && out.dir.is_none() => {
                if std::path::Path::new(other).is_dir() {
                    out.dir = Some(other.to_owned());
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn main() {
    olo_crash::install();
    let _ = tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))).try_init();

    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &raw_args) {
        return;
    }

    let cli = parse_args(&raw_args);
    let restored_raw = olo_kit::restored_state();
    let restored: Option<SavedAppState> = restored_raw.as_deref().and_then(|s| serde_json::from_str::<SavedAppState>(s).ok());
    // (Not connected to as Rex starts: what the person let go of before an
    // upgrade, and the source of a move Rex closed in the middle of: its
    // Rex would start again there, mid-move.)
    let mut let_go = restored.as_ref().map(|a| a.let_go.clone()).unwrap_or_default();
    for held in moving::held_hosts() {
        if !let_go.contains(&held) {
            let_go.push(held);
        }
    }
    hub::set_let_go(let_go);
    hub::set_helps(restored.as_ref().map(|a| a.help.clone()).unwrap_or_default());
    hub::set_saved_forwards(restored.as_ref().map(|a| a.forwards.clone()).unwrap_or_default());
    let restored_workspaces: Vec<SavedWorkspaceState> = restored.map(|a| a.workspaces).unwrap_or_default();

    // Register multi-window in-place upgrade hook so windows & sessions survive Olo upgrades!
    olo_kit::on_upgrade(|per_window| {
        let workspaces: Vec<SavedWorkspaceState> = per_window.into_iter().filter_map(|s| serde_json::from_str::<SavedWorkspaceState>(&s).ok()).collect();
        // (The hub taken from the app's lock before its own is.)
        let forwards = ui::app_hub().map(|hub| hub.saved_forwards()).unwrap_or_default();
        if workspaces.is_empty() { None } else { serde_json::to_string(&SavedAppState { workspaces, let_go: hub::let_go(), help: hub::helps(), forwards }).ok() }
    });

    let mut settings = Settings::load();
    // The hosts, what each may do here and the notices are the desktop's
    // (Preferences ▸ Rex): read before anything is connected. The first
    // time, what Rex's own file held of them is carried over; the
    // settings are told on a thread of their own, and this start goes by
    // the same merged values meanwhile.
    if let Some(desktop) = olo_kit_base::settings::current() {
        let rex = shared::adopt(&mut settings, desktop.rex);
        shared::apply(&rex, &mut settings);
        settings.save();
    }

    // (Its window's thread at the shell's tier, as Terminal's is: a key's
    // echo is drawn there. Its I/O threads raise themselves: hub.rs.)
    olo_system::priority::raise_self_patiently(olo_protocol::performance::SHELL_NICE);
    olo_kit::run(APP_ID, move |app_cx| {
        instance::listen(APP_ID, olo_kit::proxy());

        olo_kit::message_handler(app_cx, |msg, _| {
            // (The kit has taken the launch, or it wouldn't be here: not
            // asked again. Asked a second time it could be told "no"
            // after the launcher had been told "yes", and the launch
            // went nowhere: a second `olo-rex --hosts`, the palette's
            // jump to a pane.)
            if let Some(launch) = msg.downcast_ref::<Launch>() {
                let req = parse_args(&launch.args);
                if let Some((h, s, b)) = req.focus_block {
                    ui::focus_block_from_cli(h, s, b);
                } else if let Some(host) = req.focus_host {
                    ui::show_target(attention::Target::Host(host), None);
                } else if let Some((h, s)) = req.session {
                    ui::focus_session_from_cli(h, s);
                } else if req.open_settings {
                    ui::open_settings_from_cli();
                } else if req.open_hosts {
                    ui::open_panel_from_cli(ui::WinRole::Hosts);
                } else if req.open_diagnostics {
                    ui::open_panel_from_cli(ui::WinRole::Diagnostics);
                } else if req.open_qr {
                    ui::open_panel_from_cli(ui::WinRole::Access);
                } else if req.new_window {
                    ui::open_workspace_window(None);
                } else if let Some(dir) = req.dir {
                    ui::create_session_from_cli(dir);
                } else {
                    ui::open_workspace_window(None);
                }
            }
        });

        ui::init_app(settings.clone(), cli.dir.clone(), restored_workspaces.clone());
        // And followed from then on: a host added or removed in
        // Preferences appears or goes at once, a permission withdrawn
        // stops the next request.
        olo_kit_base::settings::watch(|s| ui::follow_desktop_settings(&s.rex));

        if !restored_workspaces.is_empty() {
            for ws in restored_workspaces {
                ui::open_workspace_window(Some(ws));
            }
        } else {
            ui::open_workspace_window(None);
        }
        // (A move Rex closed in the middle of, shown again: gone on, or
        // gone back from.)
        if moving::resume_unfinished() {
            ui::open_panel_from_cli(ui::WinRole::Move);
        }

        if let Some((h, s, b)) = cli.focus_block {
            ui::focus_block_from_cli(h, s, b);
        } else if let Some(host) = cli.focus_host {
            ui::show_target(attention::Target::Host(host), None);
        } else if let Some((h, s)) = cli.session {
            ui::focus_session_from_cli(h, s);
        }
        if cli.open_settings {
            ui::open_settings_from_cli();
        }
        if cli.open_hosts {
            ui::open_panel_from_cli(ui::WinRole::Hosts);
        }
        if cli.open_diagnostics {
            ui::open_panel_from_cli(ui::WinRole::Diagnostics);
        }
        if cli.open_qr {
            ui::open_panel_from_cli(ui::WinRole::Access);
        }
        if let Ok(mode) = std::env::var("OLO_REX_DEMO_RUN")
            && !mode.trim().is_empty()
        {
            ui::open_demo_run_approve(mode.trim());
        }
        if let Ok(mode) = std::env::var("OLO_REX_DEMO_CARRY")
            && !mode.trim().is_empty()
        {
            ui::open_demo_carry();
        }
        if let Ok(mode) = std::env::var("OLO_REX_DEMO_ACTIVITY")
            && !mode.trim().is_empty()
        {
            ui::open_demo_activity(mode.trim());
        }
    });
}
