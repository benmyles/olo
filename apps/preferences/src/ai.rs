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
    let keys = shortcuts::by_id("smart-tile").map(|i| shortcuts::keys_for(i, &s.keyboard.profile)).unwrap_or_default();
    let how = match keys.first() {
        Some(k) => format!("{} looks at the screen and what each window shows, and tiles them: what you’re working on bigger, what you glance at to the side.", k.label(s.keyboard.profile.platform)),
        None => "Smart Tile (in the command palette) looks at the screen and what each window shows, and tiles them.".into(),
    };
    BBox::new("Smart Tiling").show(ui, |ui| {
        feature(ui, "smart-tiling", &mut s.ai.smart_tiling, "Arrange windows with a model");
        ui.add(StringView::new(&how).dim(true).wrap(true));
    });
    BBox::new("Agent Screens").show(ui, |ui| {
        ui.add(StringView::new("Each agent screen keeps what was done on it and the frames it drew, to replay or export as a video. The oldest goes first.").dim(true).wrap(true));
        let mut size = HISTORY_MB.iter().position(|&(mb, _)| mb == s.ai.screen_history_mb).unwrap_or(2);
        let sizes: Vec<&str> = HISTORY_MB.iter().map(|(_, n)| *n).collect();
        if ui.add(MenuField::new("screen-history-size", &mut size, &sizes).label("Keep up to:").divider(94.0)).changed() {
            s.ai.screen_history_mb = HISTORY_MB[size].0;
        }
        let mut age = HISTORY_DAYS.iter().position(|&(d, _)| d == s.ai.screen_history_days).unwrap_or(2);
        let ages: Vec<&str> = HISTORY_DAYS.iter().map(|(_, n)| *n).collect();
        if ui.add(MenuField::new("screen-history-age", &mut age, &ages).label("For:").divider(94.0)).changed() {
            s.ai.screen_history_days = HISTORY_DAYS[age].0;
        }
    });
}

/// How much of each agent screen's history to keep, and for how long.
const HISTORY_MB: [(u32, &str); 5] = [(128, "128 MB a screen"), (256, "256 MB a screen"), (512, "512 MB a screen"), (1024, "1 GB a screen"), (2048, "2 GB a screen")];
const HISTORY_DAYS: [(u32, &str); 5] = [(1, "A day"), (7, "A week"), (30, "A month"), (90, "Three months"), (0, "For ever")];

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
