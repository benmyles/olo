//! Keyboard-driven settings and model-profile editor; no exact identifiers to type.
use crate::{
    client::{Action, Handle, Update, View},
    models,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use olo_coder_core::{ModelConfig, Provider, profiles::ModelProfile};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListState, Paragraph},
};
use serde_json::json;
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Panel {
    Entry {
        key: String,
        title: String,
        description: String,
        fields: Vec<crate::form::Field>,
        submission: Box<crate::form::Submission>,
        at: usize,
        status: String,
        busy: bool,
    },
    Menu {
        title: String,
        items: Vec<(String, String)>,
        selected: usize,
        #[serde(default)]
        query: String,
    },
    Profile(Box<Editor>),
    Fields {
        section: String,
        config: serde_json::Value,
        session: String,
        values: Vec<String>,
        at: usize,
        status: String,
        #[serde(default)]
        request: Option<String>,
    },
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Editor {
    profile: ModelProfile,
    fields: Vec<String>,
    at: usize,
    ids: Vec<String>,
    status: String,
    busy: bool,
    source: String,
    #[serde(default)]
    picker: Option<(String, usize)>,
    #[serde(default)]
    save_request: Option<String>,
}
impl Panel {
    pub fn resume(&mut self) {
        if let Self::Fields { request, .. } = self {
            *request = None;
        }
        if let Self::Profile(e) = self {
            e.busy = false;
            e.save_request = None;
        }
        if let Self::Entry { key, busy, .. } = self {
            *busy = false;
            *key = olo_coder_core::id();
        }
    }
    pub fn settings(view: &View) -> Self {
        let mut items = vec![
            ("Primary model".into(), "@models:primary".into()),
            ("Supervisor model".into(), "@models:supervisor".into()),
            ("Approval reviewer model".into(), "@models:reviewer".into()),
            ("Compaction model".into(), "@models:compaction".into()),
            ("Model profiles…".into(), "@profiles".into()),
            ("Start subagent…".into(), "@agent".into()),
            ("Reasoning".into(), "@reasoning".into()),
            ("Approval policy".into(), "@approvals".into()),
            ("MCP connections".into(), "@connections".into()),
            ("Message another session…".into(), "@message".into()),
        ];
        items.extend(crate::options::SECTIONS.iter().map(|(id, label)| ((*label).into(), format!("@section:{id}"))));
        if let Some(s) = &view.session {
            for (label, key, on) in [
                ("Supervisor", "supervisor", s.config.supervisor.enabled),
                ("Keep going", "keep", s.config.keep_going),
                ("Subagents", "agents", s.config.subagents),
                ("Read-only", "readonly", s.config.read_only),
            ] {
                items.push((format!("{label}: {}", if on { "On" } else { "Off" }), format!("/{key} {}", if on { "off" } else { "on" })));
            }
            for (label, key, on) in [("Agent Screens", "olo.screens", s.config.olo.screens), ("Terminal titles", "olo.terminal", s.config.olo.terminal)] {
                items.push((format!("{label}: {}", if on { "On" } else { "Off" }), format!("/set {key} {}", !on)));
            }
        }
        Self::Menu { title: "Settings".into(), items, selected: 0, query: String::new() }
    }
    fn menu(title: &str, items: Vec<(String, String)>) -> Self {
        Self::Menu { title: title.into(), items, selected: 0, query: String::new() }
    }
    pub fn update(&mut self, update: &Update) -> bool {
        if let (Self::Profile(e), Update::SettingsSaved { key, result }) = (&mut *self, update) {
            if e.save_request.as_ref() == Some(key) {
                e.save_request = None;
                e.busy = false;
                e.status = match result {
                    Ok(()) => "Profile saved. Select it from a model role in Settings.".into(),
                    Err(e) => e.clone(),
                };
            }
        }
        if let (Self::Fields { request, status, .. }, Update::SettingsSaved { key, result }) = (&mut *self, update) {
            if request.as_ref() == Some(key) {
                *request = None;
                match result {
                    Ok(()) => return true,
                    Err(e) => *status = e.clone(),
                }
            }
        }
        if let (Self::Entry { key, status, busy, .. }, Update::SettingsSaved { key: received, result }) = (&mut *self, update) {
            if key == received {
                *busy = false;
                match result {
                    Ok(()) => return true,
                    Err(e) => *status = e.clone(),
                }
            }
        }
        if let (Self::Entry { key, fields, submission, description, at, status, busy, .. }, Update::SettingsResult { key: received, result }) = (&mut *self, update) {
            if key == received {
                *busy = false;
                match result.clone().map_err(anyhow::Error::msg).and_then(|v| crate::form::next_choices(submission, v)) {
                    Ok((next, _, text)) => {
                        *fields = next;
                        *description = text;
                        *at = 0;
                        status.clear();
                    }
                    Err(e) => *status = e.to_string(),
                }
            }
        }
        if let Self::Profile(editor) = self {
            match update {
                Update::ProfileModels { profile, models } if profile.id == editor.profile.id => {
                    editor.busy = false;
                    if model_source(&profile.model) != editor.source() {
                        return false;
                    }
                    match models {
                        Ok(ids) => {
                            editor.ids = ids.clone();
                            editor.source = editor.source();
                            if !editor.ids.contains(&editor.fields[4]) {
                                editor.fields[4] = editor.ids.first().cloned().unwrap_or_default();
                            }
                            editor.status = format!("{} models available", ids.len());
                        }
                        Err(error) => editor.status = error.clone(),
                    }
                }
                Update::Profiles(profiles) => {
                    if let Ok(draft) = editor.draft() {
                        if profiles.iter().any(|p| p == &draft) {
                            editor.status = "Profile saved. Select it from a model role in Settings.".into();
                        }
                    }
                }
                _ => {}
            }
        }
        false
    }
    pub fn paste(&mut self, text: &str) {
        if let Self::Entry { fields, at, busy, .. } = self {
            if !*busy && !fields[*at].select {
                fields[*at].value.push_str(text);
            }
            return;
        }
        if let Self::Fields { section, values, at, request, .. } = self {
            if request.is_some() {
                return;
            }
            if crate::options::section(section)[*at].choices.is_empty() {
                values[*at].push_str(text);
            }
            return;
        }
        if let Self::Profile(e) = self {
            if !matches!(e.at, 1 | 4 | 5) {
                e.fields[e.at].push_str(&text.replace(['\n', '\r'], ""));
            }
        }
    }
    /// True closes the overlay. Settings remain event-driven while open.
    pub fn key(&mut self, key: KeyEvent, view: &View, client: &Handle) -> bool {
        if let Self::Profile(e) = self {
            if let Some((query, selected)) = &mut e.picker {
                let ids = filtered(&e.ids, query);
                match key.code {
                    KeyCode::Esc => {
                        e.picker = None;
                    }
                    KeyCode::Up => *selected = selected.saturating_sub(1),
                    KeyCode::Down => *selected = (*selected + 1).min(ids.len().saturating_sub(1)),
                    KeyCode::Enter => {
                        if let Some(id) = ids.get(*selected) {
                            e.fields[4] = (*id).clone();
                        }
                        e.picker = None;
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        *selected = 0;
                    }
                    KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                        query.push(c);
                        *selected = 0;
                    }
                    _ => {}
                }
                return false;
            }
        }
        if key.code == KeyCode::Esc {
            return true;
        }
        match self {
            Self::Entry { key: request_key, fields, submission, at, status, busy, .. } => {
                if *busy {
                    return false;
                }
                match key.code {
                    KeyCode::Up => *at = at.saturating_sub(1),
                    KeyCode::Down | KeyCode::Tab => *at = (*at + 1) % fields.len(),
                    KeyCode::Left | KeyCode::Right | KeyCode::Enter if fields[*at].select => {
                        let f = &mut fields[*at];
                        if !f.choices.is_empty() {
                            let i = f.choices.iter().position(|v| v == &f.value).unwrap_or(0);
                            let i = if key.code == KeyCode::Left { (i + f.choices.len() - 1) % f.choices.len() } else { (i + 1) % f.choices.len() };
                            f.value = f.choices[i].clone();
                        }
                    }
                    KeyCode::F(2) => {
                        if fields.iter().any(|f| f.required && f.value.trim().is_empty()) {
                            *status = "Fill in the required fields.".into();
                            return false;
                        }
                        let values = fields.iter().map(|f| f.value.clone()).collect::<Vec<_>>();
                        submission.prepare(&values);
                        let text = submission.command(values);
                        if submission.browsing() {
                            let command = serde_json::from_str(text.trim_start_matches("/api ")).unwrap();
                            *busy = true;
                            *status = "Loading choices…".into();
                            let _ = client.tx.send(Action::SettingsRequest { key: request_key.clone(), command: Box::new(command) });
                        } else {
                            *busy = true;
                            *status = "Applying…".into();
                            if client.tx.send(Action::SubmitSettings { text, key: request_key.clone() }).is_err() {
                                *busy = false;
                                *status = "The connection closed. Your draft is preserved.".into();
                            }
                        }
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) && !fields[*at].select => fields[*at].value.clear(),
                    KeyCode::Enter if !fields[*at].select && fields[*at].rows > 1 => fields[*at].value.push('\n'),
                    KeyCode::Backspace if !fields[*at].select => {
                        fields[*at].value.pop();
                    }
                    KeyCode::Char(c) if !fields[*at].select && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => fields[*at].value.push(c),
                    _ => {}
                }
            }
            Self::Menu { items, selected, query, .. } => match key.code {
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => *selected = (*selected + 1).min(menu_matches(items, query).len().saturating_sub(1)),
                KeyCode::Enter => {
                    let Some((_, command)) = menu_matches(items, query).get(*selected).map(|v| (*v).clone()) else {
                        return false;
                    };
                    if let Some(section) = command.strip_prefix("@section:") {
                        if let Some(s) = &view.session {
                            let config = json!(s.config);
                            let values = crate::options::section(section).iter().map(|f| crate::options::value(&config, f.path)).collect();
                            *self = Self::Fields { section: section.into(), config, session: s.id.clone(), values, at: 0, status: String::new(), request: None };
                        }
                    } else if let Some(role) = command.strip_prefix("@models:") {
                        let mut entries = Vec::new();
                        if role == "supervisor" {
                            entries.push(("Inherit primary model".into(), "/set supervisor.model null".into()));
                        }
                        if role == "compaction" {
                            entries.push(("Inherit primary model".into(), "/set compaction.model null".into()));
                        }
                        for p in &view.profiles {
                            entries.push((p.name.clone(), format!("/profiles apply {}", json!({"id":p.id,"role":role}))));
                        }
                        for (label, model) in models::choices(view, None) {
                            let field = match role {
                                "primary" => "model",
                                "supervisor" => "supervisor.model",
                                "reviewer" => "reviewer",
                                _ => "compaction.model",
                            };
                            entries.push((label, format!("/set {field} {}", json!(model))));
                        }
                        *self = Self::menu("Choose model", entries);
                    } else if command == "@profiles" {
                        let mut entries = vec![("Create model profile".into(), "@new-profile".into())];
                        entries.extend(view.profiles.iter().map(|p| (format!("Edit {}", p.name), format!("@edit-profile:{}", p.id))));
                        *self = Self::menu("Model profiles", entries);
                    } else if command == "@new-profile" {
                        *self = Self::Profile(Editor::new(ModelProfile { id: olo_coder_core::id(), name: "New model".into(), model: ModelConfig::default() }, false).into());
                    } else if let Some(id) = command.strip_prefix("@edit-profile:") {
                        if let Some(p) = view.profiles.iter().find(|p| p.id == id) {
                            *self = Self::Profile(Editor::new(p.clone(), true).into());
                        }
                    } else if command == "@reasoning" {
                        *self =
                            Self::menu("Reasoning", models::EFFORTS.iter().enumerate().map(|(i, label)| ((*label).into(), format!("/set model.effort {}", json!(models::effort_value(i))))).collect());
                    } else if command == "@approvals" {
                        *self = Self::menu(
                            "Approval policy",
                            [("Ask", "ask"), ("Automatic review", "auto"), ("Allow", "allow")].into_iter().map(|(label, value)| (label.into(), format!("/approval {value}"))).collect(),
                        );
                    } else if command == "@slack" || command == "@mcp" || command == "@password" || command == "@message" || command == "@agent" {
                        use crate::form::{Field, Submission};
                        let session = view.id.clone();
                        let (title, description, fields, submission) = if command == "@agent" {
                            let models = crate::models::choices(view, None);
                            let labels = std::iter::once("Inherit primary".to_owned()).chain(models.iter().map(|(name, _)| name.clone()));
                            let fields = vec![
                                Field::new("Task:", "").multiline(5),
                                Field::new("Subfolder:", "").optional(),
                                Field::new("Model:", "Inherit primary").choices(labels),
                                Field::new("Reasoning:", "Use model settings").choices(std::iter::once("Use model settings").chain(crate::models::EFFORTS)),
                                Field::new("Access:", "Read-only").choices(["Read-only", "Workspace writes"]),
                            ];
                            ("Start Subagent", "The parent’s policy and approval rules still apply.", fields, Submission::Agent { session, models })
                        } else if command == "@message" {
                            (
                                "Message Another Session",
                                "Choose an instance and search sessions by name.",
                                vec![
                                    Field::new("Instance:", "This service").choices(std::iter::once("This service".to_owned()).chain(view.peers.iter().map(|p| p.name.clone()))),
                                    Field::new("Search:", "").optional(),
                                ],
                                Submission::Recipient { dir: Default::default(), session, peer: None, items: vec![] },
                            )
                        } else if command == "@password" {
                            (
                                "Share Password Field",
                                "Search names, then select a specific field. Reading a shared value sends it to the model and transcript.",
                                vec![Field::new("Search", "").optional()],
                                Submission::Password { dir: Default::default(), session, phase: 0, items: vec![] },
                            )
                        } else if command == "@slack" {
                            (
                                "Connect Slack",
                                "Use a registered Slack app. Its secret stays in the service environment. Callback: http://127.0.0.1:8787/callback",
                                vec![Field::new("Client ID", ""), Field::new("Secret variable", "SLACK_CLIENT_SECRET")],
                                Submission::Integration { session, action: "setup_slack".into(), keys: vec!["client_id".into(), "client_secret_env".into()] },
                            )
                        } else {
                            (
                                "Add MCP Server",
                                "Local program or HTTPS URL. Enter references, never secret values.",
                                vec![
                                    Field::new("Name", ""),
                                    Field::new("Program or URL", ""),
                                    Field::new("Arguments (JSON)", "[]"),
                                    Field::new("Authentication", "none").choices(["none", "token", "password", "oauth"]),
                                    Field::new("Token reference", "").optional(),
                                    Field::new("OAuth client ID", "").optional(),
                                    Field::new("Secret variable", "").optional(),
                                ],
                                Submission::Integration {
                                    session,
                                    action: "configure_fields".into(),
                                    keys: ["name", "address", "args", "auth", "reference", "client_id", "client_secret_env"].iter().map(|s| (*s).into()).collect(),
                                },
                            )
                        };
                        *self = Self::Entry {
                            key: olo_coder_core::id(),
                            title: title.into(),
                            description: description.into(),
                            fields,
                            submission: Box::new(submission),
                            at: 0,
                            status: String::new(),
                            busy: false,
                        };
                    } else if command == "@connections" {
                        let mut entries = vec![
                            ("Add MCP server…".into(), "@mcp".into()),
                            ("Connect Slack…".into(), "@slack".into()),
                            ("Share password field…".into(), "@password".into()),
                            ("Connect Logfire (US)".into(), "/integrations setup {\"preset\":\"logfire\"}".into()),
                            ("Connect Logfire (EU)".into(), "/integrations setup {\"preset\":\"logfire-eu\"}".into()),
                        ];
                        for server in view.session.iter().flat_map(|s| &s.config.mcp) {
                            entries.push((server.name.clone(), format!("@connection:{}", server.name)));
                        }
                        *self = Self::menu("Connections", entries);
                    } else if let Some(name) = command.strip_prefix("@connection:") {
                        *self = Self::menu(
                            name,
                            ["test", "inspect", "login", "logout", "remove"].iter().map(|action| ((*action).into(), format!("/integrations {action} {}", json!({"name":name})))).collect(),
                        );
                    } else {
                        client.submit(command);
                        return true;
                    }
                }
                KeyCode::Backspace => {
                    query.pop();
                    *selected = 0;
                }
                KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                    query.push(c);
                    *selected = 0;
                }
                _ => {}
            },
            Self::Fields { section, config, session, values, at, status, request } => {
                if request.is_some() {
                    return false;
                }
                let specs = crate::options::section(section);
                match key.code {
                    KeyCode::Up => *at = at.saturating_sub(1),
                    KeyCode::Down | KeyCode::Tab => *at = (*at + 1) % specs.len(),
                    KeyCode::Left | KeyCode::Right | KeyCode::Enter if !specs[*at].choices.is_empty() => {
                        let choices = &specs[*at].choices;
                        let i = choices.iter().position(|v| *v == values[*at]).unwrap_or(0);
                        let i = if key.code == KeyCode::Left { (i + choices.len() - 1) % choices.len() } else { (i + 1) % choices.len() };
                        values[*at] = choices[i].into();
                    }
                    KeyCode::F(2) => {
                        let paths = specs.iter().map(|f| f.path.into()).collect::<Vec<_>>();
                        match crate::options::validated_changes(config, &paths, values) {
                            Ok(values) => {
                                let key = olo_coder_core::id();
                                let text = format!("/api {}", json!({"op":"configure_fields","session":session,"values":values}));
                                if client.tx.send(Action::SubmitSettings { key: key.clone(), text }).is_ok() {
                                    *request = Some(key);
                                    *status = "Applying…".into();
                                } else {
                                    *status = "Connection closed. Your draft is preserved.".into();
                                }
                            }
                            Err(e) => *status = e.to_string(),
                        }
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) && specs[*at].choices.is_empty() => values[*at].clear(),
                    KeyCode::Enter if specs[*at].rows > 1 => values[*at].push('\n'),
                    KeyCode::Backspace if specs[*at].choices.is_empty() => {
                        values[*at].pop();
                    }
                    KeyCode::Char(c) if specs[*at].choices.is_empty() && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => values[*at].push(c),
                    _ => {}
                }
            }
            Self::Profile(e) => match key.code {
                KeyCode::Up => e.at = e.at.saturating_sub(1),
                KeyCode::Down | KeyCode::Tab => e.at = (e.at + 1) % e.fields.len(),
                KeyCode::F(5) if !e.busy => match e.draft() {
                    Ok(profile) => {
                        e.busy = true;
                        e.status = "Fetching model catalog…".into();
                        let _ = client.tx.send(Action::DiscoverProfile(Box::new(profile)));
                    }
                    Err(error) => e.status = error.to_string(),
                },
                KeyCode::F(2) if !e.busy => {
                    if e.ids.is_empty() || e.source != e.source() {
                        e.status = "Fetch models with F5 before saving.".into();
                    } else {
                        match e.draft() {
                            Ok(profile) => {
                                let key = olo_coder_core::id();
                                let text = format!("/profiles save {}", json!({"profile":profile}));
                                if client.tx.send(Action::SubmitSettings { text, key: key.clone() }).is_ok() {
                                    e.busy = true;
                                    e.save_request = Some(key);
                                    e.status = "Saving profile…".into();
                                } else {
                                    e.status = "Connection closed. Your draft is preserved.".into();
                                }
                            }
                            Err(error) => e.status = error.to_string(),
                        }
                    }
                }
                KeyCode::Enter if e.at == 4 && e.source == e.source() => e.picker = Some((String::new(), 0)),
                KeyCode::Left | KeyCode::Right | KeyCode::Enter if matches!(e.at, 1 | 4 | 5) => {
                    let choices = match e.at {
                        1 => vec!["Codex · Token Pool".into(), "Custom endpoint".into()],
                        4 => {
                            if e.source == e.source() {
                                e.ids.clone()
                            } else {
                                vec![]
                            }
                        }
                        _ => models::EFFORTS.iter().map(|s| (*s).into()).collect(),
                    };
                    if !choices.is_empty() {
                        let at = choices.iter().position(|v| v == &e.fields[e.at]).unwrap_or(0);
                        let next = if key.code == KeyCode::Left { (at + choices.len() - 1) % choices.len() } else { (at + 1) % choices.len() };
                        e.fields[e.at] = choices[next].clone();
                    }
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) && !matches!(e.at, 1 | 4 | 5) => e.fields[e.at].clear(),
                KeyCode::Backspace if !matches!(e.at, 1 | 4 | 5) => {
                    e.fields[e.at].pop();
                }
                KeyCode::Char(c) if !matches!(e.at, 1 | 4 | 5) && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => e.fields[e.at].push(c),
                _ => {}
            },
        }
        false
    }
    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(Clear, area);
        match self {
            Self::Entry { title, description, fields, at, status, .. } => {
                let mut lines = vec![Line::from(description.clone()), Line::default()];
                lines.extend(fields.iter().enumerate().map(|(i, f)| {
                    Line::from(Span::styled(
                        format!("{} {:20} {}{}{}", if i == *at { "›" } else { " " }, f.label, if f.select { "‹ " } else { "" }, f.value, if f.select { " ›" } else { "" }),
                        if i == *at { Style::default().fg(Color::Black).bg(Color::LightYellow) } else { Style::default() },
                    ))
                }));
                lines.push(Line::from(status.clone()));
                frame.render_widget(
                    Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }).block(Block::bordered().title(format!("{title} · ↑↓ field · ←→ choice · F2 continue · Esc cancel"))),
                    area,
                );
            }
            Self::Menu { title, items, selected, query } => frame.render_stateful_widget(
                List::new(menu_matches(items, query).iter().map(|(label, _)| label.clone()))
                    .block(Block::bordered().title(format!("{title} · Filter: {query} · ↑↓ choose · Enter select · Esc close")))
                    .highlight_symbol("› ")
                    .highlight_style(Style::default().fg(Color::Black).bg(Color::LightYellow)),
                area,
                &mut ListState::default().with_selected(Some(*selected)),
            ),
            Self::Fields { section, values, at, status, .. } => {
                let specs = crate::options::section(section);
                let mut lines = specs
                    .iter()
                    .zip(values)
                    .enumerate()
                    .map(|(i, (field, value))| {
                        Line::from(Span::styled(
                            format!(
                                "{} {:23} {}{}{}",
                                if i == *at { "›" } else { " " },
                                field.label,
                                if field.choices.is_empty() { "" } else { "‹ " },
                                value,
                                if field.choices.is_empty() { "" } else { " ›" }
                            ),
                            if i == *at { Style::default().fg(Color::Black).bg(Color::LightYellow) } else { Style::default() },
                        ))
                    })
                    .collect::<Vec<_>>();
                lines.push(Line::from(status.clone()));
                frame.render_widget(
                    Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }).block(Block::bordered().title("Session Settings · ↑↓ field · ←→ choice · F2 save · Ctrl+U clear · Esc close")),
                    area,
                );
            }
            Self::Profile(e) => {
                if let Some((query, selected)) = &e.picker {
                    frame.render_stateful_widget(
                        List::new(filtered(&e.ids, query).into_iter().cloned())
                            .block(Block::bordered().title(format!("Choose model · Filter: {query} · Enter select · Esc back")))
                            .highlight_symbol("› ")
                            .highlight_style(Style::default().fg(Color::Black).bg(Color::LightYellow)),
                        area,
                        &mut ListState::default().with_selected(Some(*selected)),
                    );
                    return;
                }
                let labels = ["Name", "Provider", "API root", "Key variable", "Model", "Reasoning", "Context tokens", "Output tokens"];
                let mut lines = vec![Line::from("Choose values with ← →. Enter on Model opens a searchable list."), Line::default()];
                for (i, label) in labels.iter().enumerate() {
                    let choice = matches!(i, 1 | 4 | 5);
                    let value = if i == 4 && e.source != e.source() { "Fetch model catalog first" } else { &e.fields[i] };
                    lines.push(Line::from(Span::styled(
                        format!("{} {label:15} {}{value}{}", if i == e.at { "›" } else { " " }, if choice { "‹ " } else { "" }, if choice { " ›" } else { "" }),
                        if i == e.at { Style::default().fg(Color::Black).bg(Color::LightYellow) } else { Style::default() },
                    )));
                }
                lines.extend([Line::default(), Line::from(e.status.clone())]);
                frame.render_widget(Paragraph::new(lines).block(Block::bordered().title("Model Profile · F5 fetch models · F2 save · Esc close")).wrap(ratatui::widgets::Wrap { trim: false }), area);
            }
        }
    }
}
fn model_source(model: &ModelConfig) -> String {
    format!("{:?}\n{}\n{}", model.provider, model.endpoint.as_ref().map(|e| e.base_url.as_str()).unwrap_or(""), model.endpoint.as_ref().and_then(|e| e.api_key_env.as_deref()).unwrap_or(""))
}
impl Editor {
    fn new(profile: ModelProfile, existing: bool) -> Self {
        let m = &profile.model;
        let fields = vec![
            profile.name.clone(),
            if m.provider == Provider::Codex { "Codex · Token Pool" } else { "Custom endpoint" }.into(),
            m.endpoint.as_ref().map(|e| e.base_url.clone()).unwrap_or_default(),
            m.endpoint.as_ref().and_then(|e| e.api_key_env.clone()).unwrap_or_default(),
            m.model.clone(),
            models::EFFORTS[models::effort(&m.effort)].into(),
            m.context_window.to_string(),
            m.max_output_tokens.to_string(),
        ];
        let source = model_source(m);
        let ids = if existing { vec![m.model.clone()] } else { vec![] };
        Self { profile, fields, at: 0, ids, status: "Fetch the catalog, choose a model and save.".into(), busy: false, source, picker: None, save_request: None }
    }
    fn source(&self) -> String {
        let provider = if self.fields[1] == "Custom endpoint" { Provider::Openai } else { Provider::Codex };
        format!("{provider:?}\n{}\n{}", if provider == Provider::Openai { &self.fields[2] } else { "" }, if provider == Provider::Openai { &self.fields[3] } else { "" })
    }
    fn draft(&self) -> anyhow::Result<ModelProfile> {
        let mut p = self.profile.clone();
        p.name = self.fields[0].trim().into();
        let provider = if self.fields[1] == "Custom endpoint" { Provider::Openai } else { Provider::Codex };
        if p.model.provider != provider {
            p.model = ModelConfig::for_provider(provider);
        }
        p.model.model = self.fields[4].clone();
        p.model.effort = if self.fields[5] == "Provider default" { String::new() } else { self.fields[5].clone() };
        p.model.context_window = self.fields[6].parse()?;
        p.model.max_output_tokens = self.fields[7].parse()?;
        if let Some(endpoint) = &mut p.model.endpoint {
            endpoint.base_url = self.fields[2].trim().into();
            endpoint.api_key_env = (!self.fields[3].trim().is_empty()).then(|| self.fields[3].trim().into());
        }
        p.validate()?;
        Ok(p)
    }
}

fn filtered<'a>(ids: &'a [String], query: &str) -> Vec<&'a String> {
    let query = query.to_lowercase();
    ids.iter().filter(|v| v.to_lowercase().contains(&query)).collect()
}
fn menu_matches<'a>(items: &'a [(String, String)], query: &str) -> Vec<&'a (String, String)> {
    let query = query.to_lowercase();
    items.iter().filter(|v| v.0.to_lowercase().contains(&query)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_results_are_discarded_when_endpoint_changes() {
        let profile = ModelProfile { id: olo_coder_core::id(), name: "Endpoint".into(), model: ModelConfig::for_provider(Provider::Openai) };
        let mut panel = Panel::Profile(Editor::new(profile.clone(), false).into());
        if let Panel::Profile(e) = &mut panel {
            e.fields[2] = "https://other.example/v1".into();
        }
        panel.update(&Update::ProfileModels { profile: Box::new(profile), models: Ok(vec!["wrong-provider-model".into()]) });
        let Panel::Profile(e) = panel else { unreachable!() };
        assert!(e.ids.is_empty());
    }
    #[test]
    fn settings_restore_exact_drafts_and_paste_cannot_change_choices() {
        let mut panel = Panel::Profile(Editor::new(ModelProfile { id: olo_coder_core::id(), name: "Draft".into(), model: ModelConfig::default() }, false).into());
        if let Panel::Profile(e) = &mut panel {
            e.fields[6] = "unfinished".into();
            e.at = 1;
            e.busy = true;
        }
        panel.paste("invalid-provider");
        let mut restored: Panel = serde_json::from_str(&serde_json::to_string(&panel).unwrap()).unwrap();
        restored.resume();
        let Panel::Profile(e) = restored else { unreachable!() };
        assert_eq!(e.fields[6], "unfinished");
        assert_eq!(e.fields[1], "Codex · Token Pool");
        assert!(!e.busy);
        assert_eq!(filtered(&["vendor/Flash".into(), "Other".into()], "flash").len(), 1);
    }
}

#[cfg(test)]
mod acknowledgements {
    use super::*;
    #[test]
    fn rejected_settings_keep_the_draft_and_unrelated_acknowledgements_are_ignored() {
        let mut panel = Panel::Fields {
            section: "budgets".into(),
            config: json!(olo_coder_core::Config::default()),
            session: "session".into(),
            values: vec!["77".into()],
            at: 0,
            status: "Applying…".into(),
            request: Some("request".into()),
        };
        assert!(!panel.update(&Update::SettingsSaved { key: "other".into(), result: Ok(()) }));
        assert!(!panel.update(&Update::SettingsSaved { key: "request".into(), result: Err("Stop the active turn first".into()) }));
        if let Panel::Fields { values, status, request, .. } = &panel {
            assert_eq!(values, &["77"]);
            assert_eq!(status, "Stop the active turn first");
            assert!(request.is_none());
        }
    }
}
