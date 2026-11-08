//! ⌘↩: five emoji for a description ("pat on the back for going above
//! and beyond"), from the model chosen for Emoji in Preferences ▸ AI,
//! reached as Smart Tiling reaches its (crates/ai). Asked on a thread of
//! its own; the picker keeps working meanwhile.

use olo_protocol::{Client, Reply, Request};
use serde_json::{Value, json};

const SYSTEM: &str = "You pick emoji. Given a description of a feeling, a reaction or a \
situation, answer with exactly five different emoji that say it best, the best first, \
each a single emoji as Unicode writes it (a ZWJ sequence or flag counts as one), with a \
few words on why. Prefer emoji people commonly use for it; vary them so each offers \
something the others don't.";

/// One suggestion: the emoji as the model wrote it, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub emoji: String,
    pub why: String,
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "emoji": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "emoji": {"type": "string"},
                        "why": {"type": "string"}
                    },
                    "required": ["emoji", "why"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["emoji"],
        "additionalProperties": false
    })
}

/// The model's name, if the feature is on (asked of app_server, which
/// keeps the settings).
pub fn model() -> Result<String, String> {
    let settings = settings()?;
    if !settings.ai.emoji.enabled {
        return Err("Emoji suggestions are off in Preferences ▸ AI.".into());
    }
    Ok(settings.ai.emoji.model_name().to_owned())
}

fn settings() -> Result<olo_protocol::Settings, String> {
    let mut client = Client::connect().map_err(|e| e.to_string())?;
    match client.request(&Request::GetSettings) {
        Ok(Reply::Settings { settings }) => Ok(settings),
        Ok(_) => Err("app_server didn't give its settings".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Ask for five emoji for `description` (blocking: a worker calls it).
pub fn suggest(description: &str) -> Result<Vec<Suggestion>, String> {
    let settings = settings()?;
    let feature = &settings.ai.emoji;
    if !feature.enabled {
        return Err("Emoji suggestions are off in Preferences ▸ AI.".into());
    }
    let ask = olo_ai::Ask { system: SYSTEM.into(), content: vec![olo_ai::text_block(description)], schema: schema(), max_tokens: 4000, history: Vec::new() };
    let answer = olo_ai::ask(&settings.ai, feature, &ask).map_err(|e| e.to_string())?;
    Ok(read(&answer))
}

/// The suggestions in the answer: emoji Unicode knows, each once.
pub fn read(answer: &Value) -> Vec<Suggestion> {
    let mut out: Vec<Suggestion> = Vec::new();
    for item in answer["emoji"].as_array().into_iter().flatten() {
        let Some(e) = item["emoji"].as_str().map(str::trim) else { continue };
        let Some(known) = emojis::get(e).or_else(|| emojis::get(&format!("{e}\u{fe0f}"))) else { continue };
        if out.iter().any(|s| s.emoji == known.as_str()) {
            continue;
        }
        out.push(Suggestion { emoji: known.as_str().to_owned(), why: item["why"].as_str().unwrap_or_default().trim().to_owned() });
    }
    out.truncate(5);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_known_emoji_once() {
        let answer = json!({"emoji": [
            {"emoji": "👏", "why": "applause"},
            {"emoji": "not an emoji", "why": "?"},
            {"emoji": "👏", "why": "again"},
            {"emoji": "🙌🏽", "why": "raised hands, toned"},
            {"emoji": "❤", "why": "missing its variation selector"},
            {"emoji": "🏆", "why": "a trophy"},
            {"emoji": "🌟", "why": "a star"},
            {"emoji": "🚀", "why": "beyond"},
        ]});
        let got: Vec<String> = read(&answer).into_iter().map(|s| s.emoji).collect();
        assert_eq!(got, ["👏", "🙌🏽", "❤️", "🏆", "🌟"]);
    }
}
