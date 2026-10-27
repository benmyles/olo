//! Workspaces (how many) and Backgrounds (each workspace's desktop colour,
//! and an optional wallpaper), as in BeOS's Workspaces and Backgrounds.

use olo_kit::{
    egui::{Color32, Sense, Ui, vec2},
    metrics as m, style,
    icons,
    wallpapers::{self, Wallpaper},
    widgets::{BBox, Button, ColorControl, MenuField, Slider, TextControl, Thumbnail, ThumbnailPicker, color_well},
};
use olo_protocol::Settings;

pub const MAX_WORKSPACES: u32 = 9;

pub fn ui(ui: &mut Ui, s: &mut Settings) {
    let mut n = s.workspace_count as f32;
    let text = format!("{n:.0}");
    ui.add(
        Slider::new(&mut n, 1.0..=MAX_WORKSPACES as f32)
            .step(1.0)
            .label("Workspaces")
            .value_text(text)
            .limits("1", "9"),
    );
    s.workspace_count = n as u32;
    ui.add_space(m::SPACING);
    // A miniature of the Workspaces applet: one cell per workspace, in its colour.
    let cell = vec2(28.0, 20.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for i in 0..s.workspace_count {
            let [r, g, b] = s.workspace_color(i);
            let (rect, _) = ui.allocate_exact_size(cell, Sense::hover());
            ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(r, g, b));
            style::sunken(ui.painter(), rect, &style::palette(ui.ctx()), false);
        }
    });
}

#[derive(Default)]
pub struct BackgroundsState {
    workspace: usize,
    wallpaper: Option<String>,
    /// The setting the text was last in step with: a different one came
    /// from elsewhere (Choose…, Defaults, Revert) and replaces the text.
    seen: Option<Option<String>>,
    /// Choose… was clicked: main opens a file panel.
    pub choose: bool,
    /// The wallpaper pack, read once.
    pack: Option<Vec<Wallpaper>>,
}

/// The wallpaper previews' size (16:9).
const PREVIEW: olo_kit::egui::Vec2 = vec2(96.0, 54.0);

/// Picture formats app_server can show as a wallpaper.
pub const WALLPAPER_TYPES: &[&str] = &["png", "jpg", "jpeg", "jxl", "avif", "heic", "heif", "webp", "bmp", "tif", "tiff", "gif"];

pub fn backgrounds_ui(ui: &mut Ui, s: &mut Settings, st: &mut BackgroundsState) {
    let names: Vec<String> = (1..=s.workspace_count).map(|i| format!("Workspace {i}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    st.workspace = st.workspace.min(names.len().saturating_sub(1));
    ui.add(MenuField::new("bg-workspace", &mut st.workspace, &names).label("Workspace:").divider(80.0));

    // Make sure every workspace has its own entry before editing one.
    while s.workspace_colors.len() <= st.workspace {
        let c = s.workspace_color(s.workspace_colors.len() as u32);
        s.workspace_colors.push(c);
    }
    BBox::new("Desktop Colour").show(ui, |ui| {
        ui.horizontal(|ui| {
            let [r, g, b] = s.workspace_colors[st.workspace];
            let h = 3.0 * m::CONTROL_HEIGHT + 2.0 * m::TIGHT;
            color_well(ui, Color32::from_rgb(r, g, b), vec2(h, h));
            let ramps = (ui.available_width() - m::SPACING - 96.0).clamp(160.0, 260.0);
            ui.add(ColorControl::new(("desktop-colour", st.workspace), &mut s.workspace_colors[st.workspace]).ramp_width(ramps));
        });
    });

    BBox::new("Wallpaper").show(ui, |ui| {
        let pack = st.pack.get_or_insert_with(wallpapers::pack).clone();
        let dark = s.appearance.effective_theme == olo_kit::theme::ThemeMode::Dark;
        let own = s.wallpaper.as_deref().filter(|w| !w.starts_with(wallpapers::PACK)).map(str::to_owned);
        // None, the pack (previews in the current mode), and your own.
        // None: just this workspace's colour.
        let [r, g, b] = s.workspace_colors[st.workspace];
        let mut items = vec![Thumbnail { texture: None, color: Some(Color32::from_rgb(r, g, b)), label: "None".into() }];
        for w in &pack {
            let texture = wallpapers::thumbnail(&w.name, dark).and_then(|p| icons::thumbnail(ui.ctx(), &p, PREVIEW.x));
            items.push(Thumbnail { texture, color: None, label: w.label.clone() });
        }
        let own_texture = own.as_deref().and_then(|p| icons::thumbnail(ui.ctx(), std::path::Path::new(p), PREVIEW.x));
        items.push(Thumbnail { texture: own_texture, color: None, label: "Your Own".into() });
        let selected = match s.wallpaper.as_deref() {
            None => Some(0),
            Some(w) => match pack.iter().position(|p| p.setting() == w) {
                Some(i) => Some(i + 1),
                None => Some(items.len() - 1),
            },
        };
        match ThumbnailPicker::new("wallpaper", &items, selected).picture_size(PREVIEW).show(ui) {
            Some(0) => s.wallpaper = None,
            Some(i) if i <= pack.len() => s.wallpaper = Some(pack[i - 1].setting()),
            // Your Own: the picture already chosen, else pick one.
            Some(_) => match &own {
                Some(p) => s.wallpaper = Some(p.clone()),
                None => st.choose = true,
            },
            None => {}
        }
        ui.add_space(m::SPACING);
        // Your own picture, by name.
        if st.seen.as_ref() != Some(&own) {
            st.wallpaper = Some(own.clone().unwrap_or_default());
            st.seen = Some(own.clone());
        }
        let text = st.wallpaper.get_or_insert_with(String::new);
        ui.horizontal(|ui| {
            let field = ui.available_width() - m::BUTTON_MIN_WIDTH - m::SPACING;
            ui.add(TextControl::new(text).label("Your own:").hint("A picture of your own").width(field));
            st.choose |= ui.add(Button::new("Choose…")).clicked();
        });
        let trimmed = text.trim();
        let typed = (!trimmed.is_empty()).then(|| trimmed.to_owned());
        if typed != own && typed.is_some() {
            s.wallpaper = typed.clone();
            st.seen = Some(typed);
        }
    });
}
