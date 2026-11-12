//! Small forms, shared by the native dialogs and the terminal's editors. What a
//! form submits is a normal API command, so review, effects and audit history
//! are the same as from the terminal and automation interfaces.

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
