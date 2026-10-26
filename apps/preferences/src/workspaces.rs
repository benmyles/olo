//! Workspaces (how many) and Backgrounds (each workspace's desktop colour,
//! and an optional wallpaper), as in BeOS's Workspaces and Backgrounds.

use tabula_kit::{
    egui::{Color32, Sense, Ui, vec2},
    metrics as m, style,
    widgets::{BBox, Button, MenuField, Slider, TextControl, color_well},
};
use tabula_protocol::Settings;

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
}

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
            color_well(ui, Color32::from_rgb(r, g, b), vec2(72.0, 72.0));
            ui.vertical(|ui| {
                let rgb = &mut s.workspace_colors[st.workspace];
                for (i, (label, fill)) in [
                    ("Red", Color32::from_rgb(200, 40, 40)),
                    ("Green", Color32::from_rgb(40, 160, 40)),
                    ("Blue", Color32::from_rgb(40, 70, 200)),
                ]
                .into_iter()
                .enumerate()
                {
                    let mut v = rgb[i] as f32;
                    let text = format!("{v:.0}");
                    ui.add(Slider::new(&mut v, 0.0..=255.0).step(1.0).label(label).value_text(text).fill(fill));
                    rgb[i] = v as u8;
                }
            });
        });
    });

    BBox::new("Wallpaper").show(ui, |ui| {
        let text = st.wallpaper.get_or_insert_with(|| s.wallpaper.clone().unwrap_or_default());
        let mut clear = false;
        ui.horizontal(|ui| {
            let field = ui.available_width() - m::BUTTON_MIN_WIDTH - m::SPACING;
            ui.add(TextControl::new(text).label("Image:").hint("No wallpaper").width(field));
            ui.add_enabled_ui(!text.trim().is_empty(), |ui| clear = ui.add(Button::new("Clear")).clicked());
        });
        if clear {
            text.clear();
        }
        let trimmed = text.trim();
        let wanted = (!trimmed.is_empty()).then(|| trimmed.to_owned());
        if wanted != s.wallpaper {
            s.wallpaper = wanted;
        }
    });
    // Defaults/Revert change the wallpaper underneath us: follow it.
    if st.wallpaper.as_deref().map(str::trim).filter(|t| !t.is_empty()) != s.wallpaper.as_deref() {
        st.wallpaper = Some(s.wallpaper.clone().unwrap_or_default());
    }
}
