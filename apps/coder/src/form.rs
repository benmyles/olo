//! Small native forms. The result is a normal API command, so review, effects
//! and audit history are identical to the terminal and automation interfaces.
use crate::client::Handle;
use olo_ui::{
    WindowOptions,
    gpui::{self, AppContext as _, BorrowAppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _, Window, div, px},
    metrics as m,
    widgets::{self, Button, StringView, TextControl, TextField, TextFieldEvent},
};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Field {
    pub label: String,
    pub value: String,
    pub rows: usize,
    pub required: bool,
    #[serde(default)]
    pub choices: Vec<String>,
    #[serde(default)]
    pub select: bool,
}
impl Field {
    pub fn new(label: &'static str, value: impl Into<String>) -> Self {
        Self { label: label.into(), value: value.into(), rows: 1, required: true, choices: vec![], select: false }
    }
    pub fn choices(mut self, choices: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.select = true;
        self.choices = choices.into_iter().map(Into::into).collect();
        if !self.choices.contains(&self.value) {
            self.value = self.choices.first().cloned().unwrap_or_default();
        }
        self
    }
    pub fn multiline(mut self, rows: usize) -> Self {
        self.rows = rows;
        self
    }
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub enum Submission {
    Recipient {
        dir: std::path::PathBuf,
        session: String,
        peer: Option<String>,
        items: Vec<serde_json::Value>,
    },
    Password {
        dir: std::path::PathBuf,
        session: String,
        phase: u8,
        items: Vec<serde_json::Value>,
    },
    Settings {
        session: String,
        config: olo_coder_core::Config,
        paths: Vec<String>,
    },
    Coordination {
        operation: String,
        session: String,
        message: Option<String>,
        #[serde(default)]
        recipients: Vec<(String, String)>,
    },
    Repository {
        operation: String,
        session: String,
        head: String,
    },
    Agent {
        session: String,
        #[serde(default)]
        models: Vec<(String, olo_coder_core::ModelConfig)>,
    },
    Endpoint {
        session: String,
        config: olo_coder_core::Config,
    },
    Guidance {
        session: String,
        config: olo_coder_core::Config,
    },
    Supervisor {
        session: String,
        config: olo_coder_core::supervisor::Config,
        primary_model: olo_coder_core::ModelConfig,
        #[serde(default)]
        models: Vec<(String, olo_coder_core::ModelConfig)>,
    },
    Integration {
        session: String,
        action: String,
        keys: Vec<String>,
    },
}
impl Submission {
    pub(crate) fn browsing(&self) -> bool {
        matches!(self, Self::Password { phase: 0 | 1, .. }) || matches!(self,Self::Recipient { items, .. } if items.is_empty())
    }
    pub(crate) fn prepare(&mut self, values: &[String]) {
        if let Self::Recipient { peer, items, .. } = self {
            if items.is_empty() {
                *peer = (values[0] != "This service").then(|| values[0].clone());
            }
        }
    }
    pub(crate) fn command(&self, v: Vec<String>) -> String {
        let command = match self {
            Self::Recipient { session, peer, items, .. } => {
                if items.is_empty() {
                    if v[0] == "This service" {
                        serde_json::json!({"op":"browse","filter":{"query":v[1],"limit":500}})
                    } else {
                        serde_json::json!({"op":"peer_sessions","peer":v[0],"filter":{"query":v[1],"limit":500}})
                    }
                } else {
                    let recipient = items.iter().find(|i| i["choice"].as_str() == Some(&v[0])).expect("selected recipient");
                    serde_json::json!({"op":"send_message","session":session,"target":recipient["id"],"peer":peer,"text":v[1],"delivery":v[2],"wake":false})
                }
            }
            Self::Password { session, phase, items, .. } => {
                let (action, arguments) = match phase {
                    0 => ("password_search", serde_json::json!({"query":v[0]})),
                    1 => {
                        let i = items.iter().find(|i| i["choice"].as_str() == Some(&v[0])).expect("selected item");
                        ("password_fields", serde_json::json!({"vault":i["vault"],"item":i["id"]}))
                    }
                    _ => {
                        let f = items.iter().find(|i| i["choice"].as_str() == Some(&v[0])).expect("selected field");
                        ("grant_secret", serde_json::json!({"name":v[1],"reference":f["reference"]}))
                    }
                };
                serde_json::json!({"op":"integrations","session":session,"action":action,"arguments":arguments})
            }
            Self::Settings { session, config, paths } => {
                let config = serde_json::to_value(config).unwrap();
                let values: serde_json::Map<String, serde_json::Value> = paths
                    .iter()
                    .zip(&v)
                    .map(|(path, value)| {
                        let original = config.pointer(&crate::options::pointer(path)).unwrap();
                        (path.clone(), if original.is_string() { value.clone().into() } else { serde_json::from_str(value).unwrap_or_else(|_| value.clone().into()) })
                    })
                    .filter(|(path, value)| config.pointer(&crate::options::pointer(path)) != Some(value))
                    .collect();
                serde_json::json!({"op":"configure_fields","session":session,"values":values})
            }
            Self::Coordination { operation, session, message, recipients } => match operation.as_str() {
                "rename" => serde_json::json!({"op":"organize","session":session,"title":v[0]}),
                "connect" => serde_json::json!({"op":"link_peer","name":v[0],"directory":v[1],"reciprocal":true}),
                "edit" => serde_json::json!({"op":"edit_message","session":session,"message":message,"text":v[0]}),
                _ => {
                    let recipient = recipients.iter().find(|(label, _)| label == &v[0]).map(|(_, id)| id.as_str()).unwrap_or(&v[0]);
                    let (peer, target) = recipient.split_once('/').map(|(p, t)| (Some(p), t)).unwrap_or((None, recipient));
                    serde_json::json!({"op":"send_message","session":session,"target":target,"peer":peer,"text":v[1],"delivery":v.get(2).map(String::as_str).unwrap_or("after_tool"),"wake":false})
                }
            },
            Self::Repository { operation, session, head } => {
                let (name, arguments) = match operation.as_str() {
                    "git_commit" => ("repository_write", serde_json::json!({"operation":"commit","message":v[0],"expected_head":head})),
                    "git_branch" => ("repository_write", serde_json::json!({"operation":"branch","branch":v[0]})),
                    "git_worktree" => ("repository_write", serde_json::json!({"operation":"worktree_add","branch":v[0],"path":v[1]})),
                    _ => ("github_write", serde_json::json!({"operation":"create_pull","repository":v[0],"body":{"title":v[1],"head":v[2],"base":v[3],"body":v[4],"draft":true}})),
                };
                serde_json::json!({"op":"tool","session":session,"name":name,"arguments":arguments})
            }
            Self::Agent { session, models } => {
                let mut arguments = serde_json::json!({"task":v[0],"read_only":v.get(4).is_none_or(|v| v != "Workspace writes")});
                if !v[1].trim().is_empty() {
                    arguments["workspace"] = v[1].trim().into();
                }
                if let Some((_, model)) = models.iter().find(|(label, _)| label == &v[2]) {
                    arguments["model_config"] = serde_json::to_value(model).unwrap();
                } else if models.is_empty() && !v[2].is_empty() && v[2] != "Inherit primary" {
                    arguments["model"] = v[2].clone().into();
                }
                if v[3] != "Use model settings" && !v[3].is_empty() {
                    arguments["effort"] = if v[3] == "Provider default" { "" } else { &v[3] }.into();
                }
                serde_json::json!({"op":"tool","session":session,"name":"spawn_agent","arguments":arguments})
            }
            Self::Endpoint { session, config } => {
                let mut config = config.clone();
                if config.model.provider != olo_coder_core::Provider::Openai {
                    config.model = olo_coder_core::ModelConfig::for_provider(olo_coder_core::Provider::Openai);
                }
                let endpoint = config.model.endpoint.get_or_insert_with(Default::default);
                endpoint.base_url = v[0].trim().into();
                endpoint.api_key_env = (!v[2].trim().is_empty()).then(|| v[2].trim().to_owned());
                config.model.model = v[1].trim().into();
                config.model.effort = v[3].trim().into();
                serde_json::json!({"op":"configure","session":session,"config":config})
            }
            Self::Supervisor { session, config, primary_model, models } => {
                let mut config = serde_json::to_value(config).unwrap();
                config["guidance"] = v[0].clone().into();
                let selected = models.iter().find(|(label, _)| label == &v[1]).map(|(_, m)| m.clone());
                config["model"] = selected.map(|m| serde_json::to_value(m).unwrap()).unwrap_or(serde_json::Value::Null);
                if v[2] != "Use model settings" && !v[2].is_empty() {
                    if config["model"].is_null() {
                        config["model"] = serde_json::to_value(primary_model).unwrap();
                    }
                    config["model"]["effort"] = if v[2] == "Provider default" { "" } else { &v[2] }.into();
                }
                for (key, value) in ["every_turns", "max_interventions", "token_budget"].iter().zip(&v[3..]) {
                    config[*key] = serde_json::from_str(value).unwrap_or_else(|_| value.clone().into());
                }
                serde_json::json!({"op":"supervisor","session":session,"config":config})
            }
            Self::Integration { session, action, keys } => {
                let arguments: serde_json::Map<String, serde_json::Value> = keys.iter().cloned().zip(v.into_iter().map(Into::into)).collect();
                serde_json::json!({"op":"integrations","session":session,"action":action,"arguments":arguments})
            }
            Self::Guidance { session, .. } => serde_json::json!({"op":"configure_fields","session":session,"values":{"approval_guidance":v[0]}}),
        };
        format!("/api {command}")
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    #[serde(default = "olo_coder_core::id")]
    window_key: String,
    title: String,
    description: String,
    action: String,
    fields: Vec<Field>,
    submission: Submission,
}
#[derive(Default)]
struct Forms(Vec<gpui::WeakEntity<Form>>);
impl gpui::Global for Forms {}
pub fn snapshots(cx: &gpui::App) -> Vec<Snapshot> {
    cx.try_global::<Forms>()
        .into_iter()
        .flat_map(|forms| &forms.0)
        .filter_map(|weak| weak.upgrade())
        .map(|form| {
            let form = form.read(cx);
            Snapshot {
                window_key: form.window_key.clone(),
                title: form.title.clone(),
                description: form.description.clone(),
                action: form.action.clone(),
                submission: form.submission.clone(),
                fields: form.fields.iter().map(|(spec, field)| Field { value: field.read(cx).text().to_owned(), ..spec.clone() }).collect(),
            }
        })
        .collect()
}
pub fn restore(cx: &mut gpui::App, states: Vec<Snapshot>, client: Handle) {
    for state in states {
        open_snapshot(cx, state, client.clone());
    }
}
struct Form {
    window_key: String,
    fields: Vec<(Field, Entity<TextField>)>,
    client: Handle,
    title: String,
    action: String,
    description: String,
    submission: Submission,
    error: Option<String>,
    busy: bool,
    _subscriptions: Vec<gpui::Subscription>,
}
impl Form {
    fn next_choices(&mut self, result: serde_json::Value, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let (fields, action, description) = next_choices(&mut self.submission, result)?;
        self.action = action;
        self.description = description;
        self._subscriptions.clear();
        self.fields = fields
            .into_iter()
            .map(|spec| {
                let field = widgets::text_field(cx, |f| {
                    let f = f.name(spec.label.trim_end_matches(':').to_owned());
                    if spec.rows > 1 { f.multiline(false) } else { f }
                });
                field.update(cx, |f, cx| f.set_text(&spec.value, cx));
                self._subscriptions.push(cx.subscribe(&field, |s, _, e, cx| {
                    if matches!(e, TextFieldEvent::Changed) {
                        s.error = None;
                        cx.notify();
                    }
                }));
                (spec, field)
            })
            .collect();
        Ok(())
    }
    fn ready(&self, cx: &Context<Self>) -> bool {
        !self.busy && self.fields.iter().all(|(spec, field)| !spec.required || !field.read(cx).text().trim().is_empty())
    }
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ready(cx) {
            let values: Vec<String> = self.fields.iter().map(|(_, f)| f.read(cx).text().to_owned()).collect();
            self.submission.prepare(&values);
            let command = self.submission.command(values.clone());
            if let Submission::Password { dir, .. } | Submission::Recipient { dir, .. } = &self.submission {
                if self.submission.browsing() {
                    let dir = dir.clone();
                    let command: olo_coder_core::Command = serde_json::from_str(command.trim_start_matches("/api ")).unwrap();
                    self.busy = true;
                    self.error = Some("Loading choices…".into());
                    for (_, f) in &self.fields {
                        f.update(cx, |f, cx| f.set_enabled(false, cx));
                    }
                    cx.notify();
                    cx.spawn(async move |this, cx| {
                        let result = cx.background_spawn(async move { olo_coder_core::rpc::call(&dir, &command) }).await;
                        let _ = this.update(cx, |s, cx| {
                            s.busy = false;
                            for (_, f) in &s.fields {
                                f.update(cx, |f, cx| f.set_enabled(true, cx));
                            }
                            match result.and_then(|result| s.next_choices(result, cx)) {
                                Ok(()) => s.error = None,
                                Err(e) => s.error = Some(e.to_string()),
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                    return;
                }
            }
            let validation = (|| -> anyhow::Result<()> {
                let typed: olo_coder_core::Command = serde_json::from_str(command.trim_start_matches("/api "))?;
                match typed {
                    olo_coder_core::Command::Supervisor { config: Some(config), .. } => config.validate()?,
                    olo_coder_core::Command::Configure { config, .. } => config.validate()?,
                    olo_coder_core::Command::ConfigureFields { values, .. } => {
                        if let Submission::Settings { config, .. } = &self.submission {
                            let mut config = serde_json::to_value(config)?;
                            for (path, value) in values {
                                *config.pointer_mut(&crate::options::pointer(&path)).unwrap() = value;
                            }
                            serde_json::from_value::<olo_coder_core::Config>(config)?.validate()?;
                        }
                    }
                    _ => {}
                }
                if let Submission::Integration { action, .. } = &self.submission {
                    if action == "configure_fields" {
                        serde_json::from_str::<Vec<String>>(&values[2])?;
                    }
                }
                Ok(())
            })();
            if let Err(error) = validation {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
            let (reply, received) = tokio::sync::oneshot::channel();
            if self.client.tx.send(crate::client::Action::SubmitForm { text: command, reply }).is_err() {
                self.error = Some("The service connection closed. Your draft is preserved.".into());
                cx.notify();
                return;
            }
            self.busy = true;
            self.error = Some("Applying…".into());
            for (_, f) in &self.fields {
                f.update(cx, |f, cx| f.set_enabled(false, cx));
            }
            let handle = window.window_handle();
            cx.notify();
            cx.spawn(async move |this, cx| {
                let result = received.await.unwrap_or_else(|_| Err("The connection closed. Check session status before retrying.".into()));
                let accepted = result.is_ok();
                if this
                    .update(cx, |s, cx| {
                        s.busy = false;
                        s.error = result.err();
                        for (_, f) in &s.fields {
                            f.update(cx, |f, cx| f.set_enabled(true, cx));
                        }
                        cx.notify();
                    })
                    .is_ok()
                    && accepted
                {
                    let _ = cx.update_window(handle, |_, window, _| window.remove_window());
                }
            })
            .detach();
        }
    }
}
impl Render for Form {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let submit = cx.entity();
        let mut content = div()
            .flex()
            .flex_col()
            .p(px(m::WINDOW_INSET))
            .gap(px(m::SPACING))
            .capture_key_down(cx.listener(|s, e: &gpui::KeyDownEvent, w, cx| {
                if e.keystroke.key == "escape" {
                    w.remove_window();
                    cx.stop_propagation();
                } else if e.keystroke.key == "enter" && (e.keystroke.modifiers.platform || e.keystroke.modifiers.control || s.fields.iter().any(|(spec, f)| spec.rows == 1 && f.read(cx).is_focused(w)))
                {
                    s.submit(w, cx);
                    cx.stop_propagation();
                }
            }))
            .child(StringView::new(self.description.clone()).dim(true).wrap(true));
        for (spec, field) in &self.fields {
            if !spec.select && spec.choices.is_empty() {
                content = content
                    .child(TextControl::new(field.clone()).label(spec.label.clone()).divider(if matches!(self.submission, Submission::Settings { .. }) { 164.0 } else { 120.0 }).rows(spec.rows));
            } else {
                let choices = spec.choices.clone();
                let field = field.clone();
                let selected = choices.iter().position(|v| v == field.read(cx).text()).unwrap_or(0);
                content = content.child(
                    widgets::MenuField::new(spec.label.clone(), if choices.is_empty() { vec!["None available".into()] } else { choices.iter().cloned().map(Into::into).collect() }, selected)
                        .enabled(!self.busy && !choices.is_empty())
                        .label(spec.label.clone())
                        .divider(if matches!(self.submission, Submission::Settings { .. }) { 164.0 } else { 120.0 })
                        .on_change(move |i, _, cx| field.update(cx, |f, cx| f.replace_all(&choices[i], cx))),
                );
            }
        }
        if let Some(error) = &self.error {
            content = content.child(StringView::new(error.clone()).wrap(true));
        }
        content.child(
            div()
                .flex()
                .justify_end()
                .gap(px(m::SPACING))
                .child(Button::new("Cancel").on_click(|w, _| w.remove_window()))
                .child(Button::new(self.action.clone()).default(true).enabled(self.ready(cx)).on_click(move |w, cx| submit.update(cx, |s, cx| s.submit(w, cx)))),
        )
    }
}
pub fn open(cx: &mut gpui::App, title: &str, description: &'static str, action: &'static str, fields: Vec<Field>, client: Handle, submission: Submission) {
    open_snapshot(cx, Snapshot { window_key: olo_coder_core::id(), title: title.into(), description: description.into(), action: action.into(), fields, submission }, client);
}
fn open_snapshot(cx: &mut gpui::App, snapshot: Snapshot, client: Handle) {
    if cx.try_global::<Forms>().is_none() {
        cx.set_global(Forms::default());
    }
    cx.defer(move |cx| {
        let Snapshot { window_key, title, description, action, fields, submission } = snapshot;
        olo_ui::open_window(cx, WindowOptions::new(title.clone(), 580.0, 200.0).dialog().child().restore_key(window_key.clone()), move |window, cx| {
            let view = cx.new(|cx| {
                let fields: Vec<_> = fields
                    .into_iter()
                    .map(|spec| {
                        let field = widgets::text_field(cx, |f| {
                            let f = f.name(spec.label.trim_end_matches(':').to_owned());
                            if spec.rows > 1 { f.multiline(false) } else { f }
                        });
                        field.update(cx, |f, cx| f.set_text(&spec.value, cx));
                        (spec, field)
                    })
                    .collect();
                let subscriptions = fields
                    .iter()
                    .map(|(_, f)| {
                        cx.subscribe(f, |s: &mut Form, _, e, cx| {
                            if matches!(e, TextFieldEvent::Changed) {
                                s.error = None;
                                cx.notify();
                            }
                        })
                    })
                    .collect();
                Form { window_key, fields, client, title, action, description, submission, error: None, busy: false, _subscriptions: subscriptions }
            });
            cx.update_global::<Forms, _>(|forms, _| {
                forms.0.retain(|weak| weak.upgrade().is_some());
                forms.0.push(view.downgrade());
            });
            if let Some((_, field)) = view.read(cx).fields.first() {
                field.read(cx).focus(window);
            }
            view
        });
    });
}

pub(crate) fn next_choices(submission: &mut Submission, result: serde_json::Value) -> anyhow::Result<(Vec<Field>, String, String)> {
    if let Submission::Recipient { session, items, .. } = submission {
        let mut found = result["sessions"].as_array().cloned().ok_or_else(|| anyhow::anyhow!("No session catalog returned"))?;
        found.retain(|r| r["id"].as_str() != Some(session));
        anyhow::ensure!(!found.is_empty(), "No matching sessions. Try another search or instance.");
        for row in &mut found {
            row["choice"] = format!("{} · {}", row["title"].as_str().unwrap_or("Untitled"), row["id"].as_str().unwrap_or("")).into();
        }
        let fields = vec![
            Field::new("Recipient:", "").choices(found.iter().map(|r| r["choice"].as_str().unwrap().to_owned())),
            Field::new("Message:", "").multiline(6),
            Field::new("Delivery:", "after_tool").choices(["after_tool", "queue", "interrupt"]),
        ];
        *items = found;
        return Ok((fields, "Send".into(), "Choose the recipient and when to deliver the message. Sending does not wake a paused session.".into()));
    }
    let Submission::Password { phase, items, .. } = submission else { unreachable!() };
    let key = if *phase == 0 { "items" } else { "fields" };
    let mut found = result[key].as_array().cloned().or_else(|| result["found"].as_array().cloned()).ok_or_else(|| anyhow::anyhow!("Unlock Passwords to select an item."))?;
    anyhow::ensure!(!found.is_empty(), "No matches. Try another search.");
    for item in &mut found {
        let choice = if *phase == 0 {
            format!("{} · {} · {}", item["title"].as_str().unwrap_or("Item"), item["subtitle"].as_str().unwrap_or(""), item["id"].as_str().unwrap_or(""))
        } else {
            format!("{}{} · {}", item["section"].as_str().map(|s| format!("{s} / ")).unwrap_or_default(), item["label"].as_str().unwrap_or("Field"), item["id"].as_str().unwrap_or(""))
        };
        item["choice"] = choice.into();
    }
    let mut fields = vec![Field::new(if *phase == 0 { "Item:" } else { "Field:" }, "").choices(found.iter().map(|i| i["choice"].as_str().unwrap().to_owned()))];
    if *phase == 1 {
        fields.push(Field::new("Agent-facing name:", ""));
    }
    let action: String = if *phase == 0 { "Choose Item" } else { "Share Field" }.into();
    let description: String = if *phase == 0 {
        "Choose an item to see its field names. No secret values are read."
    } else {
        "Only the selected field is shared with this session. Reading it sends its value to the model and saves it in the transcript."
    }
    .into();
    *phase += 1;
    *items = found;
    Ok((fields, action, description))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn decode(s: &Submission, values: &[&str]) -> serde_json::Value {
        serde_json::from_str(s.command(values.iter().map(|s| (*s).into()).collect()).trim_start_matches("/api ")).unwrap()
    }
    #[test]
    fn model_selection_preserves_custom_endpoint_and_inheritance() {
        let mut model = olo_coder_core::ModelConfig::for_provider(olo_coder_core::Provider::Openai);
        model.model = "vendor/fast-model".into();
        model.endpoint.as_mut().unwrap().base_url = "https://api.example.org/v1".into();
        let s = Submission::Agent { session: "session".into(), models: vec![("Fast".into(), model.clone())] };
        let request = decode(&s, &["Task", "", "Fast", "Use model settings"]);
        assert_eq!(request["arguments"]["model_config"], json!(model));
        assert!(request["arguments"].get("effort").is_none());
        let inherited = decode(&s, &["Task", "", "Inherit primary", "Provider default"]);
        assert!(inherited["arguments"].get("model_config").is_none());
        assert_eq!(inherited["arguments"]["effort"], "");
    }
    #[test]
    fn password_picker_only_grants_selected_reference() {
        let mut submission = Submission::Password { dir: Default::default(), session: "session".into(), phase: 0, items: vec![] };
        let (items, _, _) = next_choices(&mut submission, json!({"items":[{"vault":"vault","id":"item","title":"Example","subtitle":"Login"}]})).unwrap();
        let request = decode(&submission, &[&items[0].value]);
        assert_eq!(request["action"], "password_fields");
        assert_eq!(request["arguments"], json!({"vault":"vault","item":"item"}));
        let (fields, _, _) = next_choices(
            &mut submission,
            json!({"fields":[{"id":"password","label":"Password","reference":"op://vault/item/password"},{"id":"token","label":"Token","reference":"op://vault/item/token"}]}),
        )
        .unwrap();
        let grant = decode(&submission, &[&fields[0].choices[1], "deployment-token"]);
        assert_eq!(grant["action"], "grant_secret");
        assert_eq!(grant["arguments"], json!({"name":"deployment-token","reference":"op://vault/item/token"}));
    }
    #[test]
    fn empty_choices_remain_selectors_and_defaults_are_valid() {
        let field = Field::new("Choice", "missing").choices(["one", "two"]);
        assert_eq!(field.value, "one");
        let empty = Field::new("Choice", "").choices(Vec::<String>::new());
        assert!(empty.select && empty.choices.is_empty());
    }
}
