//! AI: Olo's AI features, each on or off, with its model (OpenAI's,
//! through the Token Pool's Codex accounts) and effort. Smart Tiling is
//! the first.

use olo_kit::{
    egui::Ui,
    widgets::{BBox, CheckBox, MenuField, StringView},
};
use olo_protocol::{AI_EFFORTS, AI_MODELS, AiFeature, Settings, shortcuts};

pub fn ui(ui: &mut Ui, s: &mut Settings) {
    BBox::new("Models").show(ui, |ui| {
        ui.add(StringView::new("AI features use OpenAI’s models through the Token Pool’s Codex accounts, spent in its order.").dim(true).wrap(true));
    });
    let keys = shortcuts::by_id("smart-tile").map(|i| shortcuts::keys_for(i, &s.shortcuts)).unwrap_or_default();
    let how = match keys.first() {
        Some(k) => format!("{} looks at the screen and what each window shows, and tiles them: what you’re working on bigger, what you glance at to the side.", k.symbols()),
        None => "Smart Tile (in the command palette) looks at the screen and what each window shows, and tiles them.".into(),
    };
    BBox::new("Smart Tiling").show(ui, |ui| {
        feature(ui, "smart-tiling", &mut s.ai.smart_tiling, "Arrange windows with a model");
        ui.add(StringView::new(&how).dim(true).wrap(true));
    });
}

/// A feature's switch, model and effort.
fn feature(ui: &mut Ui, id: &str, f: &mut AiFeature, label: &str) {
    ui.add(CheckBox::new(&mut f.enabled, label));
    ui.add_enabled_ui(f.enabled, |ui| {
        let models: Vec<&str> = AI_MODELS.iter().map(|(_, name)| *name).collect();
        let mut m = AI_MODELS.iter().position(|(id, _)| *id == f.model_id()).unwrap_or(0);
        if ui.add(MenuField::new((id, "model"), &mut m, &models).label("Model:").divider(70.0)).changed() {
            f.model = AI_MODELS[m].0.into();
        }
        let efforts: Vec<&str> = AI_EFFORTS.iter().map(|(_, name)| *name).collect();
        let mut e = AI_EFFORTS.iter().position(|(id, _)| *id == f.effort).unwrap_or(1);
        if ui.add(MenuField::new((id, "effort"), &mut e, &efforts).label("Thinking:").divider(70.0)).changed() {
            f.effort = AI_EFFORTS[e].0.into();
        }
    });
}
