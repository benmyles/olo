//! The Wallpaper portal (`org.freedesktop.impl.portal.Wallpaper`): an
//! app's "Set as Desktop Background" (a photo viewer's, a browser's
//! image menu, a sandboxed app's). When the app asks to show a preview,
//! Olo asks first, as a BAlert with the picture; then the picture is
//! copied into Olo's backgrounds folder (so it stays when the app's copy
//! goes) and becomes the wallpaper, as Preferences ▸ Backgrounds would
//! set it (app_server's `Settings::wallpaper`).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use olo_kit::{
    Cx, WindowId, WindowOptions,
    egui::{self, Rect, pos2, vec2},
    icons, metrics as m,
    style::{self, c},
    widgets,
};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use crate::portal::ToUi;

const WIDTH: u32 = 400;
const ICON: &str = "backgrounds";
const TITLE: &str = "Set as Desktop Background?";
/// The picture's box, under the question.
const PICTURE_H: f32 = 150.0;
const COLUMN: f32 = WIDTH as f32 - widgets::ALERT_TEXT_X - m::WINDOW_INSET;

/// What the dialog was asked.
pub struct Ask {
    pub request: String,
    pub parent: String,
    pub app: String,
    pub picture: PathBuf,
    /// The answer: set it, or not.
    pub reply: async_channel::Sender<bool>,
}

pub struct Wallpaper {
    pub to_ui: std::sync::Arc<dyn Fn(ToUi) + Send + Sync>,
}

/// Where Olo keeps backgrounds set from other programs.
fn backgrounds() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/share")))?;
    Some(data.join("olo").join("backgrounds"))
}

/// Copy `picture` into the backgrounds folder (under its own name, made
/// unique) and make it the wallpaper.
fn set(picture: &Path) -> Result<(), String> {
    let dir = backgrounds().ok_or("no home folder")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = picture.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "background".into());
    let kept = olo_xdg::trash::unique(&dir, &name);
    std::fs::copy(picture, &kept).map_err(|e| format!("{}: {e}", picture.display()))?;
    let mut c = olo_protocol::Client::connect().map_err(|e| e.to_string())?;
    let olo_protocol::Reply::Settings { mut settings } = c.request(&olo_protocol::Request::GetSettings).map_err(|e| e.to_string())? else {
        return Err("app_server didn't give its settings".into());
    };
    settings.wallpaper = Some(kept.display().to_string());
    c.request(&olo_protocol::Request::SetSettings { settings }).map_err(|e| e.to_string())?;
    Ok(())
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Wallpaper")]
impl Wallpaper {
    #[zbus(name = "SetWallpaperURI")]
    async fn set_wallpaper_uri(
        &self,
        handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        uri: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> u32 {
        let picture = match olo_xdg::open::Target::parse(&uri) {
            olo_xdg::open::Target::Path(p) if p.is_file() => p,
            _ => return 2,
        };
        let preview = options.get("show-preview").and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
        if preview {
            let (tx, rx) = async_channel::bounded(1);
            let request = crate::portal::RequestObject { handle: handle.to_string(), to_ui: self.to_ui.clone() };
            let _ = server.at(handle.as_ref(), request).await;
            (self.to_ui)(ToUi::Wallpaper(Ask { request: handle.to_string(), parent: parent_window, app: crate::app_name(&app_id), picture: picture.clone(), reply: tx }));
            let yes = rx.recv().await.unwrap_or(false);
            let _ = server.remove::<crate::portal::RequestObject, _>(handle.as_ref()).await;
            if !yes {
                return 1;
            }
        }
        match blocking::unblock(move || set(&picture)).await {
            Ok(()) => 0,
            Err(e) => {
                tracing::warn!("portal: couldn't set the background: {e}");
                2
            }
        }
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

/// The question on screen.
pub struct Dialog {
    ask: Ask,
}

fn body(ask: &Ask) -> String {
    let who = if ask.app.is_empty() { "An app".to_owned() } else { format!("“{}”", ask.app) };
    format!("{who} would like to make this picture your desktop background.")
}

fn height(ask: &Ask) -> u32 {
    let text = m::LABEL_ROW + m::TIGHT + style::plain_text_height(&body(ask), COLUMN) + m::SPACING;
    (2.0 * m::WINDOW_INSET + text + PICTURE_H + m::SPACING + m::CONTROL_HEIGHT + m::WINDOW_INSET).ceil() as u32
}

impl Dialog {
    pub fn open(cx: &mut Cx, ask: Ask) -> (WindowId, Dialog) {
        let size = (WIDTH, height(&ask));
        let mut options = WindowOptions { size, min_size: Some(size), max_size: Some(size), ..WindowOptions::alert("Desktop Background", "") };
        if !ask.parent.is_empty() {
            options = options.foreign_parent(ask.parent.clone());
        }
        let id = cx.open_window(options);
        cx.activate(id);
        (id, Dialog { ask })
    }

    pub fn request(&self) -> &str {
        &self.ask.request
    }

    pub fn answer(&self, yes: bool) {
        let _ = self.ask.reply.try_send(yes);
    }

    /// Draw it; Some(yes) once answered.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<bool> {
        let p = style::palette(ui.ctx());
        let col = widgets::alert_frame(ui, ICON);
        let mut y = col.top();
        style::label(ui.painter(), col.left(), y + style::PLAIN_SIZE / 2.0 + 2.0, egui::Align::Min, TITLE, style::bold_font(), c(p.panel_text));
        y += m::LABEL_ROW + m::TIGHT;
        y = widgets::alert_text(ui, col, y, &body(&self.ask)) + m::SPACING;
        // The picture, fitted (never past a texel to a device pixel), with
        // a hairline around it.
        let bounds = Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), PICTURE_H));
        match icons::thumbnail(ui.ctx(), &self.ask.picture, PICTURE_H.max(col.width())) {
            Some(tex) => {
                let at = icons::fit(ui.ctx(), &tex, bounds);
                ui.painter().image(tex.id(), at, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
                ui.painter().rect_stroke(at, 0.0, egui::Stroke::new(1.0 / ui.ctx().pixels_per_point(), c(p.control_border)), egui::StrokeKind::Outside);
                style::describe_text(ui.ctx(), egui::accesskit::Role::Image, &self.ask.picture.display().to_string(), at, None);
            }
            None => {
                style::label(ui.painter(), bounds.center().x, bounds.center().y, egui::Align::Center, "", style::plain_font(), style::disabled_text(&p));
            }
        }
        widgets::alert_buttons(ui, col, &["Cancel", "Set Background"], true).map(|i| i == 1)
    }
}
