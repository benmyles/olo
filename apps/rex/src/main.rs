//! Olo Rex: native Olo desktop client for Rex ("the multiplexer for all work").
//!
//! Built on `olo-rex-vt` (`libghostty-vt`), `olo-rex-client` (Rex API rev 19 +
//! native Rust SSH + tunnels), and `olo-kit` (120 Hz GPU rendering, BeOS/Olo chrome,
//! in-place upgrade survival, Deskbar Glance, and zero-trust Olo Desktop Bridge).

mod agent;
mod bridge;
mod copy_mode;
mod git;
mod hub;
mod keymap;
mod links;
mod overlays;
mod panels;
mod settings;
mod terminal_pane;
mod themes;
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
    open_palette: bool,
    open_goto: bool,
    dir: Option<String>,
    session: Option<(String, String)>,
    focus_block: Option<(String, String, String)>,
}

fn parse_args(args: &[String]) -> CliArgs {
    let mut out = CliArgs {
        new_window: false,
        open_settings: false,
        open_hosts: false,
        open_diagnostics: false,
        open_qr: false,
        open_palette: false,
        open_goto: false,
        dir: None,
        session: None,
        focus_block: None,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--new-window" => out.new_window = true,
            "--settings" => out.open_settings = true,
            "--hosts" => out.open_hosts = true,
            "--diagnostics" => out.open_diagnostics = true,
            "--qr" => out.open_qr = true,
            "--palette" => out.open_palette = true,
            "--goto" => out.open_goto = true,
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
    let restored_workspaces: Vec<SavedWorkspaceState> = restored_raw.as_deref().and_then(|s| serde_json::from_str::<SavedAppState>(s).ok()).map(|a| a.workspaces).unwrap_or_default();

    // Register multi-window in-place upgrade hook so windows & sessions survive Olo upgrades!
    olo_kit::on_upgrade(|per_window| {
        let workspaces: Vec<SavedWorkspaceState> = per_window.into_iter().filter_map(|s| serde_json::from_str::<SavedWorkspaceState>(&s).ok()).collect();
        if workspaces.is_empty() { None } else { serde_json::to_string(&SavedAppState { workspaces }).ok() }
    });

    let settings = Settings::load();

    olo_kit::run(APP_ID, move |app_cx| {
        instance::listen(APP_ID, olo_kit::proxy());

        olo_kit::message_handler(app_cx, |msg, _| {
            if let Some(launch) = msg.downcast_ref::<Launch>() {
                if !launch.take() {
                    return;
                }
                let req = parse_args(&launch.args);
                if let Some((h, s, b)) = req.focus_block {
                    ui::focus_block_from_cli(h, s, b);
                } else if let Some((h, s)) = req.session {
                    ui::focus_session_from_cli(h, s);
                } else if req.open_settings {
                    ui::open_settings_from_cli();
                } else if req.open_hosts {
                    ui::open_panel_from_cli(ui::WinRole::Hosts);
                } else if req.open_diagnostics {
                    ui::open_panel_from_cli(ui::WinRole::Diagnostics);
                } else if req.open_qr {
                    ui::open_panel_from_cli(ui::WinRole::QrPairing);
                } else if req.open_palette {
                    ui::run_workspace_command_from_cli("app.command_palette");
                } else if req.open_goto {
                    ui::run_workspace_command_from_cli("app.go_to_anything");
                } else if req.new_window {
                    ui::open_workspace_window(None);
                } else if let Some(dir) = req.dir {
                    ui::app().hub.create_session("local", None, Some(dir));
                } else {
                    ui::open_workspace_window(None);
                }
            }
        });

        ui::init_app(settings.clone(), cli.dir.clone(), restored_workspaces.clone());

        if !restored_workspaces.is_empty() {
            for ws in restored_workspaces {
                ui::open_workspace_window(Some(ws));
            }
        } else {
            ui::open_workspace_window(None);
        }

        if let Some((h, s, b)) = cli.focus_block {
            ui::focus_block_from_cli(h, s, b);
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
            ui::open_panel_from_cli(ui::WinRole::QrPairing);
        }
        if cli.open_palette {
            ui::run_workspace_command_from_cli("app.command_palette");
        }
        if cli.open_goto {
            ui::run_workspace_command_from_cli("app.go_to_anything");
        }
    });
}
