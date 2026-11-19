//! AI: Olo's AI features, each on or off, with its model (OpenAI's,
//! through the Token Pool's Codex accounts) and effort:
//! the command palette's /ask, the emoji picker's suggestions and
//! Weather's and Calendar's assistants; and
//! the model and effort Olo starts Claude Code with for a task.

use olo_kit::SharedString;
use olo_protocol::{AI_EFFORTS, AI_MODELS, AiFeature, CLAUDE_MODELS, Settings, shortcuts};

use crate::form::Ui;

pub fn ui(ui: &mut Ui) {
    let s = ui.s.clone();
    // (Three columns, each a kind: what answers where you type (the
    // models, /ask, emoji); the apps' assistants (Weather, Calendar);
    // agents (their screens, Claude Code).)
    let typed = ui.collect(|ui| {
        ui.bbox("Models", |ui| {
            ui.note("AI features use OpenAI’s models through the Token Pool’s Codex accounts, spent in its order.");
        });
        ui.bbox("Ask", |ui| {
            feature(ui, "ask", &s.ai.ask, "Answer questions in the command palette", |s| &mut s.ai.ask);
            ui.note("In the command palette, type /ask and a question: the answer shows there with code to copy (its number, then Return). Space asks a follow-up.");
        });
        let keys = shortcuts::by_id("emoji-picker").map(|i| shortcuts::keys_for(i, &s.keyboard.profile)).unwrap_or_default();
        let how = match keys.first() {
            Some(k) => olo_kit::tr!("In the emoji picker ({}), describe what you want to say and press ⌘↩ for five emoji that say it.", k.label(s.keyboard.profile.platform)),
            None => olo_kit::words::tr("In the emoji picker, describe what you want to say and press ⌘↩ for five emoji that say it.").into(),
        };
        ui.bbox("Emoji", |ui| {
            feature(ui, "emoji", &s.ai.emoji, "Suggest emoji for a description", |s| &mut s.ai.emoji);
            ui.note(how);
        });
    });
    let assistants = ui.collect(|ui| {
        ui.bbox("Weather", |ui| {
            feature(ui, "weather", &s.ai.weather, "Answer questions about the forecast", |s| &mut s.ai.weather);
            ui.note("Weather’s assistant is handed the whole forecast for the place you’re looking at, every layer hour by hour, to answer from.");
        });
        ui.bbox("Calendar", |ui| {
            feature(ui, "calendar", &s.ai.calendar, "Answer questions about your calendars", |s| &mut s.ai.calendar);
            ui.note("Calendar’s assistant is handed the events of the calendars you show. It can draft events; nothing is added until you save it.");
        });
    });
    let agents = ui.collect(|ui| {
        ui.bbox("Agent Screens", |ui| {
            ui.note("Each agent screen keeps what was done on it and the frames it drew, to replay or export as a video. The oldest goes first.");
            let size = HISTORY_MB.iter().position(|&(mb, _)| mb == s.ai.screen_history_mb).unwrap_or(2);
            let sizes: Vec<SharedString> = HISTORY_MB.iter().map(|(_, n)| SharedString::from(*n)).collect();
            ui.menu("screen-history-size", sizes, size, |f| f.label("Keep up to:").divider(94.0), |s, i| s.ai.screen_history_mb = HISTORY_MB[i].0);
            let age = HISTORY_DAYS.iter().position(|&(d, _)| d == s.ai.screen_history_days).unwrap_or(2);
            let ages: Vec<SharedString> = HISTORY_DAYS.iter().map(|(_, n)| SharedString::from(*n)).collect();
            ui.menu("screen-history-age", ages, age, |f| f.label("For:").divider(94.0), |s, i| s.ai.screen_history_days = HISTORY_DAYS[i].0);
        });
        // (Ben, 2026-11-08: "have the claude code model selectable in the
        // ai prefs, default to opus 5.5 on medium".)
        ui.bbox("Claude Code", |ui| {
            ui.note("When Olo starts Claude Code for a task (DiskBackup’s Agent Setup), in a Terminal window you can follow, it uses this model.");
            let c = &s.ai.claude_code;
            let models: Vec<SharedString> = CLAUDE_MODELS.iter().map(|(_, name)| SharedString::from(*name)).collect();
            let m = CLAUDE_MODELS.iter().position(|(id, _)| *id == c.model).unwrap_or(0);
            ui.menu("claude-code-model", models, m, |f| f.label("Model:").divider(70.0), |s, i| s.ai.claude_code.model = CLAUDE_MODELS[i].0.into());
            let efforts: Vec<SharedString> = AI_EFFORTS.iter().map(|(_, name)| SharedString::from(*name)).collect();
            let e = AI_EFFORTS.iter().position(|(id, _)| *id == c.effort).unwrap_or(1);
            ui.menu("claude-code-effort", efforts, e, |f| f.label("Thinking:").divider(70.0), |s, i| s.ai.claude_code.effort = AI_EFFORTS[i].0.into());
        });
    });
    ui.add_columns_of(vec![typed, assistants, agents]);
}

/// How much of each agent screen's history to keep, and for how long.
const HISTORY_MB: [(u32, &str); 5] = [(128, "128 MB a screen"), (256, "256 MB a screen"), (512, "512 MB a screen"), (1024, "1 GB a screen"), (2048, "2 GB a screen")];
const HISTORY_DAYS: [(u32, &str); 5] = [(1, "A day"), (7, "A week"), (30, "A month"), (90, "Three months"), (0, "For ever")];

/// A feature's switch, model and effort; `of` finds it in the settings.
fn feature(ui: &mut Ui, id: &str, f: &AiFeature, label: &str, of: fn(&mut Settings) -> &mut AiFeature) {
    ui.check(olo_kit::words::of_text(label), f.enabled, move |s, v| of(s).enabled = v);
    ui.enabled_if(f.enabled, |ui| {
        let models: Vec<SharedString> = AI_MODELS.iter().map(|(_, name)| SharedString::from(*name)).collect();
        let m = AI_MODELS.iter().position(|(id, _)| *id == f.model_id()).unwrap_or(0);
        ui.menu(&format!("{id}-model"), models, m, |f| f.label("Model:").divider(70.0), move |s, i| of(s).model = AI_MODELS[i].0.into());
        let efforts: Vec<SharedString> = AI_EFFORTS.iter().map(|(_, name)| SharedString::from(*name)).collect();
        let e = AI_EFFORTS.iter().position(|(id, _)| *id == f.effort).unwrap_or(1);
        ui.menu(&format!("{id}-effort"), efforts, e, |f| f.label("Thinking:").divider(70.0), move |s, i| of(s).effort = AI_EFFORTS[i].0.into());
    });
}
