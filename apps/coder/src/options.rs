//! Shared settings descriptions for native forms and terminal editors.
#[derive(Clone)]
pub struct OptionSpec {
    pub path: &'static str,
    pub label: &'static str,
    pub choices: Vec<&'static str>,
    pub rows: usize,
}
/// Settings' sections, the window's and the terminal's: their ids,
/// titles and what they're for ("model" is a page of its own).
pub const SECTIONS: [(&str, &str, &str); 8] = [
    ("model", "Model", "What the agent runs on, and the models its other jobs use."),
    ("safety", "Approvals", "What the agent may do without asking you."),
    ("autonomy", "Autonomy", "How the agent keeps going, and who watches it."),
    ("compaction", "Context", "How a long conversation is kept within the model’s window. The whole conversation is always kept."),
    ("budgets", "Limits", "How far one run may go before it stops and asks."),
    ("timeouts", "Timeouts", "How long actions and requests may take, and how often they’re tried again."),
    ("collaboration", "Messages", "What other sessions and Coder instances may do."),
    ("instructions", "Instructions", "What the agent is told at the start of every turn."),
];

/// The options in section `id`, in order; a heading has no path.
pub fn section(id: &str) -> Vec<OptionSpec> {
    let toggle = |path, label| OptionSpec { path, label, choices: vec!["true", "false"], rows: 1 };
    let field = |path, label| OptionSpec { path, label, choices: vec![], rows: 1 };
    match id {
        "safety" => vec![
            OptionSpec { path: "approval", label: "Actions:", choices: vec!["ask", "auto", "allow"], rows: 1 },
            OptionSpec { path: "approval_guidance", label: "Reviewer’s guidance:", choices: vec![], rows: 5 },
            OptionSpec { path: "sandbox", label: "Files:", choices: vec!["unrestricted", "workspace"], rows: 1 },
            toggle("read_only", "Only read, never change"),
        ],
        "autonomy" => vec![
            toggle("keep_going", "Keep going toward the goal by itself"),
            heading("Subagents"),
            toggle("subagents", "Let it hand work to subagents"),
            field("max_subagents", "At once, at most:"),
            field("max_depth", "Levels deep:"),
            heading("Supervisor"),
            toggle("supervisor.enabled", "A second model watches the work"),
            OptionSpec { path: "supervisor.guidance", label: "What it looks for:", choices: vec![], rows: 5 },
            toggle("supervisor.recover_errors", "It may restart work an error stopped"),
            field("supervisor.every_turns", "Looks every:"),
            field("supervisor.max_reviews", "Looks, at most:"),
            field("supervisor.max_interventions", "Steps in, at most:"),
            field("supervisor.token_budget", "Its tokens, at most:"),
            field("supervisor.stall_seconds", "Stalled after:"),
        ],
        "compaction" => vec![
            toggle("model.auto_compact", "Summarise older context as the window fills"),
            field("model.compact_at", "Summarise at:"),
            toggle("compaction.background", "Prepare the summary ahead, while it works"),
            field("compaction.prepare_percent", "Prepare from:"),
            toggle("compaction.verify", "Check each summary against the conversation"),
            heading("Kept word for word"),
            field("compaction.keep_recent_exchanges", "Recent exchanges:"),
            field("compaction.keep_recent_tokens", "Recent tokens, up to:"),
            heading("Summaries"),
            field("compaction.chunk_tokens", "Read in pieces of:"),
            field("compaction.summary_tokens", "A summary, at most:"),
        ],
        "budgets" => vec![field("max_turns", "Turns, at most:"), field("token_budget", "Tokens, at most:"), field("max_tool_bytes", "An action’s output:"), field("max_response_bytes", "A reply:")],
        "timeouts" => vec![field("command_timeout_secs", "An action may take:"), field("request_timeout_secs", "A request may take:"), field("retries", "Tries again:")],
        "collaboration" => vec![toggle("accept_messages", "Take messages from other sessions"), toggle("wake_on_message", "A message may start it when it’s idle")],
        "instructions" => vec![
            OptionSpec { path: "instructions", label: "Instructions:", choices: vec![], rows: 8 },
            OptionSpec { path: "compaction.instructions", label: "For summaries:", choices: vec![], rows: 5 },
        ],
        _ => vec![],
    }
}

/// A heading over the options after it (no option itself).
fn heading(label: &'static str) -> OptionSpec {
    OptionSpec { path: "", label, choices: vec![], rows: 0 }
}

/// How a choice reads ("" for one that reads as it is).
pub fn choice_label(path: &str, value: &str) -> &'static str {
    match (path, value) {
        ("approval", "ask") => "Ask me first",
        ("approval", "auto") => "A reviewer decides, asks me when unsure",
        ("approval", "allow") => "Allow everything",
        ("sandbox", "workspace") => "Only the workspace folder",
        ("sandbox", "unrestricted") => "Anywhere",
        _ => "",
    }
}

/// What a number setting's value means, after its field ("64 MB", "500K
/// tokens", "10 minutes", "no limit").
pub fn unit(path: &str, text: &str) -> String {
    let Ok(n) = text.trim().replace([',', '_'], "").parse::<u64>() else { return String::new() };
    let limit = matches!(path, "max_turns" | "token_budget");
    match path {
        _ if limit && n == 0 => "no limit".into(),
        "max_turns" => "turns".into(),
        p if p.ends_with("_bytes") => bytes(n),
        p if p.ends_with("_secs") || p.ends_with("_seconds") => duration(n),
        "retries" => if n == 1 { "time" } else { "times" }.into(),
        "compaction.prepare_percent" => "% of the way".into(),
        "supervisor.every_turns" => if n == 1 { "turn" } else { "turns" }.into(),
        "max_subagents" => if n == 1 { "subagent" } else { "subagents" }.into(),
        "max_depth" => if n == 1 { "level" } else { "levels" }.into(),
        "compaction.keep_recent_exchanges" | "supervisor.max_reviews" | "supervisor.max_interventions" => String::new(),
        _ => format!("{} tokens", crate::models::tokens(n)),
    }
}

fn bytes(n: u64) -> String {
    // To a tenth: "64 MB", "1.5 MB", "512 KB".
    let tenths = |unit: u64| {
        let t = (n * 10 + unit / 2) / unit;
        if t % 10 == 0 { format!("{}", t / 10) } else { format!("{}.{}", t / 10, t % 10) }
    };
    match n {
        n if n >= 1 << 20 => format!("{} MB", tenths(1 << 20)),
        n if n >= 1 << 10 => format!("{} KB", tenths(1 << 10)),
        n => format!("{n} bytes"),
    }
}

fn duration(n: u64) -> String {
    match n {
        n if n >= 3600 && n % 3600 == 0 => format!("{} hour{}", n / 3600, if n == 3600 { "" } else { "s" }),
        n if n >= 60 && n % 60 == 0 => format!("{} minute{}", n / 60, if n == 60 { "" } else { "s" }),
        n => format!("{n} second{}", if n == 1 { "" } else { "s" }),
    }
}

pub fn pointer(path: &str) -> String {
    format!("/{}", path.replace('.', "/"))
}
pub fn value(config: &serde_json::Value, path: &str) -> String {
    let v = config.pointer(&pointer(path)).unwrap_or(&serde_json::Value::Null);
    v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())
}
pub fn changes(config: &serde_json::Value, paths: &[String], values: &[String]) -> anyhow::Result<std::collections::BTreeMap<String, serde_json::Value>> {
    let mut changes: std::collections::BTreeMap<String, serde_json::Value> = paths
        .iter()
        .zip(values)
        .map(|(path, value)| {
            let original = config.pointer(&pointer(path)).ok_or_else(|| anyhow::anyhow!("unknown setting {path}"))?;
            // A number may be written with its thousands marked (1,000,000).
            let new = match original {
                serde_json::Value::String(_) => value.clone().into(),
                serde_json::Value::Number(_) => serde_json::from_str(&value.trim().replace([',', '_'], ""))?,
                _ => serde_json::from_str(value.trim())?,
            };
            Ok((path.clone(), new))
        })
        .collect::<anyhow::Result<_>>()?;
    changes.retain(|path, value| config.pointer(&pointer(path)) != Some(value));
    Ok(changes)
}

pub fn validated_changes(config: &serde_json::Value, paths: &[String], values: &[String]) -> anyhow::Result<std::collections::BTreeMap<String, serde_json::Value>> {
    let changes = changes(config, paths, values)?;
    let mut draft = config.clone();
    for (path, value) in &changes {
        *draft.pointer_mut(&pointer(path)).ok_or_else(|| anyhow::anyhow!("unknown setting {path}"))? = value.clone();
    }
    serde_json::from_value::<olo_coder_core::Config>(draft)?.validate()?;
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_settings_form_matches_the_schema_and_validates_defaults() {
        let config = serde_json::json!(olo_coder_core::Config::default());
        for (id, _, _) in SECTIONS {
            let fields: Vec<_> = section(id).into_iter().filter(|f| !f.path.is_empty()).collect();
            let paths: Vec<_> = fields.iter().map(|f| f.path.to_owned()).collect();
            let values: Vec<_> = fields.iter().map(|f| value(&config, f.path)).collect();
            validated_changes(&config, &paths, &values).unwrap_or_else(|e| panic!("{id}: {e}"));
            for (field, value) in fields.iter().zip(&values) {
                assert!(field.choices.is_empty() || field.choices.contains(&value.as_str()), "{} default must be selectable", field.path);
            }
        }
        let changes = validated_changes(&config, &["max_turns".into(), "token_budget".into()], &[value(&config, "max_turns"), "100000".into()]).unwrap();
        assert_eq!(changes.len(), 1, "unchanged fields must not overwrite concurrent edits");
        assert_eq!(changes["token_budget"], 100000);
        assert!(validated_changes(&config, &["max_turns".into()], &["-1".into()]).is_err());
        assert!(validated_changes(&config, &["max_turns".into()], &["50".into()]).is_ok());
        assert!(validated_changes(&config, &["approval".into()], &["invented".into()]).is_err());
        assert_eq!(validated_changes(&config, &["token_budget".into()], &["1,000,000".into()]).unwrap()["token_budget"], 1_000_000);
        assert_eq!(validated_changes(&config, &["instructions".into()], &["First, this; then, that.".into()]).unwrap()["instructions"], "First, this; then, that.");
        assert_eq!(unit("max_tool_bytes", "67108864"), "64 MB");
        assert_eq!(unit("max_response_bytes", "1572864"), "1.5 MB");
        assert_eq!(unit("model.compact_at", "500000"), "500K tokens");
        assert_eq!(unit("token_budget", "0"), "no limit");
        assert_eq!(unit("command_timeout_secs", "600"), "10 minutes");
        assert_eq!(unit("retries", "1"), "time");
    }
}
