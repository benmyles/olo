//! Named models are configured once and selected by role throughout Coder.
use olo_coder_core::ModelConfig;
pub const EFFORTS: [&str; 8] = ["Provider default", "none", "low", "medium", "high", "xhigh", "max", "ultra"];
pub fn effort(value: &str) -> usize {
    EFFORTS.iter().position(|v| *v == value).unwrap_or(0)
}
pub fn effort_value(index: usize) -> String {
    if index == 0 { String::new() } else { EFFORTS[index].into() }
}
pub fn choices(view: &crate::client::View, extra: Option<(&str, &ModelConfig)>) -> Vec<(String, ModelConfig)> {
    let mut out = Vec::new();
    if let Some((label, model)) = extra {
        out.push((label.into(), model.clone()));
    }
    if let Some(s) = &view.session {
        out.push(("Current primary".into(), s.config.model.clone()));
        for id in &view.models {
            let mut model = s.config.model.clone();
            model.model = id.clone();
            out.push((format!("Model · {id}"), model));
        }
    }
    out.extend(view.profiles.iter().map(|p| (format!("Profile · {}", p.name), p.model.clone())));
    out
}
