//! AI: Olo's AI features, each on or off, with its model (OpenAI's,
//! through the Token Pool's Codex accounts) and effort: Smart Tiling,
//! and the emoji picker's suggestions.

use olo_protocol::{AI_EFFORTS, AI_MODELS, AiFeature, Settings, shortcuts};
use olo_ui::gpui::SharedString;

use crate::form::Ui;

pub fn ui(ui: &mut Ui) {
    let s = ui.s.clone();
    ui.bbox("Models", |ui| {
        ui.note("AI features use OpenAI’s models through the Token Pool’s Codex accounts, spent in its order.");
    });
    let keys = shortcuts::by_id("smart-tile").map(|i| shortcuts::keys_for(i, &s.keyboard.profile)).unwrap_or_default();
    let how = match keys.first() {
        Some(k) => format!("{} looks at the screen and what each window shows, and tiles them: what you’re working on bigger, what you glance at to the side.", k.label(s.keyboard.profile.platform)),
        None => "Smart Tile (in the command palette) looks at the screen and what each window shows, and tiles them.".into(),
    };
    ui.bbox("Smart Tiling", |ui| {
        feature(ui, "smart-tiling", &s.ai.smart_tiling, "Arrange windows with a model", |s| &mut s.ai.smart_tiling);
        ui.note(how);
    });
    let keys = shortcuts::by_id("emoji-picker").map(|i| shortcuts::keys_for(i, &s.keyboard.profile)).unwrap_or_default();
    let how = match keys.first() {
        Some(k) => format!("In the emoji picker ({}), describe what you want to say and press ⌘↩ for five emoji that say it.", k.label(s.keyboard.profile.platform)),
        None => "In the emoji picker, describe what you want to say and press ⌘↩ for five emoji that say it.".into(),
    };
    ui.bbox("Emoji", |ui| {
        feature(ui, "emoji", &s.ai.emoji, "Suggest emoji for a description", |s| &mut s.ai.emoji);
        ui.note(how);
    });
    ui.bbox("Agent Screens", |ui| {
        ui.note("Each agent screen keeps what was done on it and the frames it drew, to replay or export as a video. The oldest goes first.");
        let size = HISTORY_MB.iter().position(|&(mb, _)| mb == s.ai.screen_history_mb).unwrap_or(2);
        let sizes: Vec<SharedString> = HISTORY_MB.iter().map(|(_, n)| SharedString::from(*n)).collect();
        ui.menu("screen-history-size", sizes, size, |f| f.label("Keep up to:").divider(94.0), |s, i| s.ai.screen_history_mb = HISTORY_MB[i].0);
        let age = HISTORY_DAYS.iter().position(|&(d, _)| d == s.ai.screen_history_days).unwrap_or(2);
        let ages: Vec<SharedString> = HISTORY_DAYS.iter().map(|(_, n)| SharedString::from(*n)).collect();
        ui.menu("screen-history-age", ages, age, |f| f.label("For:").divider(94.0), |s, i| s.ai.screen_history_days = HISTORY_DAYS[i].0);
    });
}

/// How much of each agent screen's history to keep, and for how long.
const HISTORY_MB: [(u32, &str); 5] = [(128, "128 MB a screen"), (256, "256 MB a screen"), (512, "512 MB a screen"), (1024, "1 GB a screen"), (2048, "2 GB a screen")];
const HISTORY_DAYS: [(u32, &str); 5] = [(1, "A day"), (7, "A week"), (30, "A month"), (90, "Three months"), (0, "For ever")];

/// A feature's switch, model and effort; `of` finds it in the settings.
fn feature(ui: &mut Ui, id: &str, f: &AiFeature, label: &str, of: fn(&mut Settings) -> &mut AiFeature) {
    ui.check(label.to_owned(), f.enabled, move |s, v| of(s).enabled = v);
    ui.enabled_if(f.enabled, |ui| {
        let models: Vec<SharedString> = AI_MODELS.iter().map(|(_, name)| SharedString::from(*name)).collect();
        let m = AI_MODELS.iter().position(|(id, _)| *id == f.model_id()).unwrap_or(0);
        ui.menu(&format!("{id}-model"), models, m, |f| f.label("Model:").divider(70.0), move |s, i| of(s).model = AI_MODELS[i].0.into());
        let efforts: Vec<SharedString> = AI_EFFORTS.iter().map(|(_, name)| SharedString::from(*name)).collect();
        let e = AI_EFFORTS.iter().position(|(id, _)| *id == f.effort).unwrap_or(1);
        ui.menu(&format!("{id}-effort"), efforts, e, |f| f.label("Thinking:").divider(70.0), move |s, i| of(s).effort = AI_EFFORTS[i].0.into());
    });
}
