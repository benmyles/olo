//! Shared settings descriptions for native forms and terminal editors.
#[derive(Clone)]
pub struct OptionSpec {
    pub path: &'static str,
    pub label: &'static str,
    pub choices: Vec<&'static str>,
    pub rows: usize,
}
pub const SECTIONS: [(&str, &str); 7] = [
    ("budgets", "Run budgets"),
    ("timeouts", "Timeouts & retries"),
    ("compaction", "Context & compaction"),
    ("subagents", "Subagent limits"),
    ("safety", "Safety & collaboration"),
    ("supervision", "Supervisor limits"),
    ("instructions", "Instructions"),
];
pub fn section(name: &str) -> Vec<OptionSpec> {
    let field = |path, label| OptionSpec { path, label, choices: vec![], rows: 1 };
    let toggle = |path, label| OptionSpec { path, label, choices: vec!["true", "false"], rows: 1 };
    match name {
        "budgets" => vec![field("max_turns", "Maximum turns:"), field("token_budget", "Token budget:"), field("max_tool_bytes", "Tool output bytes:"), field("max_response_bytes", "Response bytes:")],
        "timeouts" => vec![field("command_timeout_secs", "Tool timeout (s):"), field("request_timeout_secs", "Request timeout (s):"), field("retries", "Retries:")],
        "compaction" => vec![
            toggle("model.auto_compact", "Auto compact:"),
            field("model.compact_at", "Compact at (tokens):"),
            toggle("compaction.background", "Background:"),
            toggle("compaction.verify", "Verify summaries:"),
            field("compaction.prepare_percent", "Prepare at (%):"),
            field("compaction.keep_recent_exchanges", "Recent exchanges:"),
            field("compaction.keep_recent_tokens", "Recent tokens:"),
            field("compaction.chunk_tokens", "Chunk tokens:"),
            field("compaction.summary_tokens", "Summary tokens:"),
        ],
        "subagents" => vec![toggle("subagents", "Enable subagents:"), field("max_subagents", "Concurrent agents:"), field("max_depth", "Delegation depth:")],
        "safety" => vec![
            OptionSpec { path: "approval", label: "Approval policy:", choices: vec!["ask", "auto", "allow"], rows: 1 },
            OptionSpec { path: "sandbox", label: "Sandbox:", choices: vec!["workspace", "unrestricted"], rows: 1 },
            toggle("read_only", "Read-only:"),
            toggle("accept_messages", "Accept messages:"),
            toggle("wake_on_message", "Allow message wake:"),
            toggle("keep_going", "Keep going:"),
        ],
        "supervision" => vec![
            toggle("supervisor.enabled", "Enable supervisor:"),
            OptionSpec { path: "supervisor.guidance", label: "Supervisor guidance:", choices: vec![], rows: 5 },
            toggle("supervisor.recover_errors", "Recover errors:"),
            field("supervisor.every_turns", "Review every (turns):"),
            field("supervisor.max_reviews", "Most reviews:"),
            field("supervisor.max_interventions", "Most interventions:"),
            field("supervisor.token_budget", "Supervisor tokens:"),
            field("supervisor.stall_seconds", "Stalled after (s):"),
        ],
        "instructions" => vec![
            OptionSpec { path: "instructions", label: "Instructions:", choices: vec![], rows: 8 },
            OptionSpec { path: "compaction.instructions", label: "Compaction guidance:", choices: vec![], rows: 5 },
        ],
        _ => vec![],
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
            let new = if original.is_string() { value.clone().into() } else { serde_json::from_str(value)? };
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
        for (id, _) in SECTIONS {
            let fields = section(id);
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
        assert!(validated_changes(&config, &["max_turns".into()], &["0".into()]).is_err());
        assert!(validated_changes(&config, &["approval".into()], &["invented".into()]).is_err());
    }
}
