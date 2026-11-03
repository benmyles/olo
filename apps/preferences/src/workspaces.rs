//! Workspaces (how many) and Backgrounds (each workspace's desktop colour,
//! and an optional wallpaper), as in BeOS's Workspaces and Backgrounds.

use olo_ui::{
    file_panel::{self, FilePanel, PanelResult},
    gpui::{Context, IntoElement as _, ParentElement as _, SharedString, Styled as _, div, px, rgb},
    metrics as m, pictures, style, theme,
    widgets::{Button, ColorControl, ColorWell, MenuField, Thumbnail, ThumbnailPicker, color_control_height},
};
use olo_xdg::wallpapers::{self, Wallpaper};

use crate::{PanelView, form::Ui, panel};

pub const MAX_WORKSPACES: u32 = 9;

pub fn ui(ui: &mut Ui) {
    let n = ui.s.workspace_count as f32;
    ui.slider("workspaces", n, 1.0..=MAX_WORKSPACES as f32, |s| s.step(1.0).label("Workspaces").value_text(format!("{n:.0}")).limits("1", "9"), |s, v| s.workspace_count = v as u32);
    ui.space(m::SPACING);
    // A miniature of the Workspaces applet: one cell per workspace, in its colour.
    let colours: Vec<[u8; 3]> = (0..ui.s.workspace_count).map(|i| ui.s.workspace_color(i)).collect();
    ui.add(div().flex().flex_row().gap(px(4.0)).children(colours.into_iter().map(|c| {
        olo_ui::gpui::canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                style::fill_rect(window, b, colour(c));
                style::sunken(window, b, &theme::palette(cx), false);
            },
        )
        .w(px(28.0))
        .h(px(20.0))
    })));
    ui.space(m::TIGHT);
    ui.note("Each display has workspaces of its own.");
}

fn colour([r, g, b]: [u8; 3]) -> olo_ui::gpui::Hsla {
    rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)).into()
}

#[derive(Default)]
pub struct BackgroundsState {
    workspace: usize,
    /// The wallpaper pack, read once.
    pack: Option<Vec<Wallpaper>>,
}

fn state_of(v: &mut PanelView) -> Option<&mut BackgroundsState> {
    match &mut v.state {
        panel::State::Backgrounds(st) => Some(st),
        _ => None,
    }
}

/// The wallpaper previews' size (16:9).
const PREVIEW: (f32, f32) = (96.0, 54.0);

/// Picture formats app_server can show as a wallpaper.
pub const WALLPAPER_TYPES: &[&str] = &["png", "jpg", "jpeg", "jxl", "avif", "heic", "heif", "webp", "bmp", "tif", "tiff", "gif"];

/// The file panel for a picture of one's own, starting where the current
/// one is, else in the Pictures folder.
fn choose(ui: &Ui) -> impl Fn(&mut olo_ui::gpui::Window, &mut olo_ui::gpui::App) + Clone + 'static {
    let prefs = ui.prefs();
    let current = ui.s.wallpaper.clone();
    move |_, cx| {
        let dir = current
            .as_deref()
            .and_then(|p| std::path::Path::new(p).parent().map(|d| d.to_owned()))
            .filter(|d| d.is_dir())
            .or_else(|| olo_xdg::user_dir("PICTURES"))
            .unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()));
        let prefs = prefs.clone();
        file_panel::show(cx, "Choose a Wallpaper", FilePanel::open(dir).types(WALLPAPER_TYPES), move |a, cx| {
            if let PanelResult::Chosen(path) = a.result {
                let _ = prefs.update(cx, |p, cx| p.edit(cx, |s| s.wallpaper = Some(path.to_string_lossy().into_owned())));
            }
        });
    }
}

pub fn backgrounds_ui(ui: &mut Ui, st: &mut BackgroundsState) {
    let s = ui.s.clone();
    let names: Vec<SharedString> = (1..=s.workspace_count).map(|i| format!("Workspace {i}").into()).collect();
    st.workspace = st.workspace.min(names.len().saturating_sub(1));
    let pick = ui.on_view(|v, i: usize, _, cx: &mut Context<PanelView>| {
        if let Some(st) = state_of(v) {
            st.workspace = i;
        }
        cx.notify();
    });
    ui.add(MenuField::new("bg-workspace", names, st.workspace).label("Workspace:").divider(80.0).on_change(pick));

    let ws = st.workspace;
    let rgb = s.workspace_color(ws as u32);
    ui.bbox("Desktop Colour", |ui| {
        // Every workspace has its own entry before one is changed.
        let set = ui.bind(rgb, move |s, c: [u8; 3]| {
            while s.workspace_colors.len() <= ws {
                let c = s.workspace_color(s.workspace_colors.len() as u32);
                s.workspace_colors.push(c);
            }
            s.workspace_colors[ws] = c;
        });
        let h = color_control_height();
        let ramps = (f32::from(ui.window.viewport_size().width) - 2.0 * m::WINDOW_INSET - 2.0 * m::BOX_INSET - h - 2.0 * m::SPACING - 96.0).clamp(160.0, 260.0);
        ui.add(div().flex().flex_row().gap(px(m::SPACING)).child(ColorWell::new(colour(rgb), h)).child(ColorControl::new(format!("desktop-colour-{ws}"), rgb).ramp_width(ramps).on_change(set)));
    });

    let pack = st.pack.get_or_insert_with(wallpapers::pack).clone();
    let choose = choose(ui);
    ui.bbox("Wallpaper", |ui| {
        let dark = s.appearance.effective_theme == olo_ui::palette::ThemeMode::Dark;
        let own = s.wallpaper.as_deref().filter(|w| !w.starts_with(wallpapers::PACK)).map(str::to_owned);
        let px_wide = (PREVIEW.0 * ui.window.scale_factor()).round() as u32;
        // None (just this workspace's colour), the pack (previews in the
        // current mode), and your own.
        let mut items = vec![Thumbnail { picture: None, color: Some(colour(rgb)), label: "None".into() }];
        for w in &pack {
            let picture = wallpapers::thumbnail(&w.name, dark).and_then(|p| pictures::thumbnail(ui.cx, &p, px_wide));
            items.push(Thumbnail { picture, color: None, label: w.label.clone().into() });
        }
        let own_picture = own.as_deref().and_then(|p| pictures::thumbnail(ui.cx, std::path::Path::new(p), px_wide));
        items.push(Thumbnail { picture: own_picture, color: None, label: "Your Own".into() });
        let selected = match s.wallpaper.as_deref() {
            None => Some(0),
            Some(w) => match pack.iter().position(|p| p.setting() == w) {
                Some(i) => Some(i + 1),
                None => Some(items.len() - 1),
            },
        };
        let n = pack.len();
        let set = ui.bind(selected.unwrap_or(0), {
            let (pack, own) = (pack.clone(), own.clone());
            move |s, i: usize| match i {
                0 => s.wallpaper = None,
                i if i <= n => s.wallpaper = Some(pack[i - 1].setting()),
                _ => {
                    if let Some(p) = &own {
                        s.wallpaper = Some(p.clone());
                    }
                }
            }
        });
        // Your Own with none chosen yet: pick one.
        let choose_own = choose_twin(&choose);
        let has_own = own.is_some();
        // As wide as the box's content: as many side by side as fit.
        let wide = f32::from(ui.window.viewport_size().width) - 2.0 * (m::WINDOW_INSET + m::BOX_INSET);
        let picker = ThumbnailPicker::new("wallpaper", items, selected).picture_size(PREVIEW.0, PREVIEW.1).width(wide).on_select(move |i, window, cx| {
            if i > n && !has_own {
                choose_own(window, cx);
            } else {
                set(i, window, cx);
            }
        });
        ui.add(picker);
        ui.space(m::SPACING);
        // Your own picture, by name.
        let field = ui.field("wallpaper-own", own.as_deref().unwrap_or(""), |f| f.hint("A picture of your own"), |s, v| {
            let t = v.trim();
            if !t.is_empty() {
                s.wallpaper = Some(t.to_owned());
            }
        });
        ui.add(
            div()
                .flex()
                .flex_row()
                .gap(px(m::SPACING))
                .child(div().flex_1().child(olo_ui::widgets::TextControl::new(field).label("Your own:")))
                .child(Button::new("Choose…").on_click(choose).into_any_element()),
        );
    });
}

/// The same file panel, for another control.
fn choose_twin(f: &(impl Fn(&mut olo_ui::gpui::Window, &mut olo_ui::gpui::App) + Clone + 'static)) -> impl Fn(&mut olo_ui::gpui::Window, &mut olo_ui::gpui::App) + 'static {
    f.clone()
}
