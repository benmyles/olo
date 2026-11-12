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
    Bounds, ClickEvent, Cx, Element, InteractiveElement as _, KeyDownEvent, ParentElement as _, Styled as _, View, WindowHandle, WindowOptions, canvas, div, metrics as m, point, px, style, theme,
    widgets::{
        AlertFrame, Button,
        alert::{ALERT_TEXT_X, alert_title, button_row, paragraph, paragraph_height},
    },
};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use crate::{
    portal::ToUi,
    ui::{self, Answer},
};

const WIDTH: f32 = 400.0;
const ICON: &str = "backgrounds";
const TITLE: &str = "Set as Desktop Background?";
/// The picture's box, under the question.
const PICTURE_H: f32 = 150.0;
const COLUMN: f32 = WIDTH - ALERT_TEXT_X - m::WINDOW_INSET;

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
    answer: Answer<bool>,
    app: String,
    picture: PathBuf,
}

fn body(app: &str) -> String {
    let who = if app.is_empty() { "An app".to_owned() } else { format!("“{app}”") };
    format!("{who} would like to make this picture your desktop background.")
}

fn height(ask: &Ask) -> f32 {
    let text = m::LABEL_ROW + m::TIGHT + paragraph_height(&body(&ask.app), COLUMN) + m::SPACING;
    (2.0 * m::WINDOW_INSET + text + PICTURE_H + m::SPACING + m::CONTROL_HEIGHT + m::WINDOW_INSET).ceil()
}

pub fn open(ask: Ask) -> WindowHandle<Dialog> {
    let options = ui::over(WindowOptions::alert("Desktop Background", WIDTH, height(&ask)), &ask.parent);
    ui::open(options, move |_| Dialog { answer: Answer::new(ask.reply, false), app: ask.app, picture: ask.picture })
}

impl Dialog {
    fn finish(&mut self, yes: bool, cx: &mut Cx<Self>) {
        self.answer.give(yes);
        cx.close_window();
    }
}

impl View for Dialog {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        // The picture, fitted (never past a texel to a device pixel), with
        // a hairline around it.
        let path = self.picture.clone();
        let picture = canvas(move |g, b| {
            let s = g.scale_factor();
            let place = ((f32::from(b.size.width) * s).round() as u32, (PICTURE_H * s).round() as u32);
            let Some(img) = olo_kit::pictures::fitted(g, &path, place) else { return };
            let isz = img.size(0);
            let at = style::snap_centred(g, b.center(), isz.width.0 as f32 / s, isz.height.0 as f32 / s);
            let _ = g.paint_image(at, Default::default(), img, 0, false);
            let p = theme::palette();
            let line = style::hairline(g);
            style::frame_rect(g, Bounds::from_corners(at.origin - point(line, line), at.bottom_right() + point(line, line)), style::c(p.control_border));
            olo_kit::agent::record(g, olo_kit::agent::Node { role: "Image", name: Some(path.display().to_string().into()), bounds: at, enabled: true, ..Default::default() });
        })
        .w_full()
        .h(px(PICTURE_H));
        let no = Button::new("Cancel").on_click(cx.listener(|d: &mut Dialog, _: &ClickEvent, cx| d.finish(false, cx)));
        // (Return sets it: the default button. Escape cancels.)
        let yes = Button::new("Set Background").default(true).on_click(cx.listener(|d: &mut Dialog, _: &ClickEvent, cx| d.finish(true, cx)));
        div()
            .size_full()
            .on_window_key_down(cx.listener(|d: &mut Dialog, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                    d.finish(false, cx);
                }
            }))
            .child(
                AlertFrame::new(ICON)
                    .child(alert_title(TITLE))
                    .child(div().mt(px(m::TIGHT)).child(paragraph(body(&self.app))))
                    .child(div().mt(px(m::SPACING)).child(picture))
                    .child(div().flex_1())
                    .child(button_row().child(no).child(yes)),
            )
    }
}
