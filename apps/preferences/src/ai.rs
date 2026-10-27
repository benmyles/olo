//! AI: how Tabula's AI features reach their models (the Token Pool, or
//! an Anthropic API key), and each feature, on or off, with its model and
//! effort. Smart Tiling is the first.

use tabula_kit::{
    egui::Ui,
    widgets::{BBox, Button, CheckBox, MenuField, StringView, TextControl},
};
use tabula_protocol::{AI_EFFORTS, AI_MODELS, AiFeature, Settings, shortcuts};

/// The API key being typed, and what saving it said.
#[derive(Default)]
pub struct State {
    key: Option<String>,
    status: Option<String>,
}

pub fn ui(ui: &mut Ui, s: &mut Settings, st: &mut State) {
    let ai = &mut s.ai;
    BBox::new("Models").show(ui, |ui| {
        ui.add(CheckBox::new(&mut ai.use_pool, "Use the Token Pool for AI features"));
        if ai.use_pool {
            ui.add(StringView::new("Models are reached with the Token Pool’s accounts, spent in its order.").dim(true).wrap(true));
        } else {
            // A few bytes from our own config folder.
            let key = st.key.get_or_insert_with(|| tabula_ai::saved_key().unwrap_or_default());
            ui.horizontal(|ui| {
                ui.add(TextControl::new(key).label("Anthropic API key:").password(true).width(ui.available_width() - tabula_kit::metrics::BUTTON_MIN_WIDTH - tabula_kit::metrics::SPACING));
                if ui.add(Button::new("Save")).clicked() {
                    st.status = Some(match tabula_ai::save_key(key) {
                        Ok(()) if key.trim().is_empty() => "Forgotten.".into(),
                        Ok(()) => "Saved.".into(),
                        Err(e) => format!("Couldn’t save it: {e}"),
                    });
                }
            });
            let note = st.status.as_deref().unwrap_or("Astra 6 is only reached through the Token Pool.");
            ui.add(StringView::new(note).dim(true).wrap(true));
        }
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
        let mut m = AI_MODELS.iter().position(|(id, _)| *id == f.model).unwrap_or(0);
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
