//! Settings: everything about the session's agent in one window, by
//! section, as Preferences is: what it runs on (the model, saved models
//! and custom endpoints), what it may do without asking, how it keeps
//! going, how its context is kept, its limits and its instructions.
//! Choices are pop-ups and switches, numbers are checked as they're typed,
//! and nothing is changed until Apply (or, for a model, Use), so a half-
//! made change is never in force. Changes go to the session through the
//! same API as the terminal's, and apply between the agent's steps.
use crate::client::{Action, Handle, View as Shown};
use olo_coder_core::{Command, ModelConfig, Provider, profiles::ModelProfile, rpc};
use olo_kit::{
    Child, Cx, Element, IntoElement as _, ParentElement as _, Styled as _, View, WindowCx, WindowOptions, canvas, div, metrics as m, px, style, theme,
    widgets::{Button, CheckBox, MenuField, Sidebar, StringView, TextControl, TextField, TextFieldEvent, sidebar::Row, text_field},
};
use serde_json::{Value, json};
use std::path::PathBuf;

/// The sections, each its title and the options in it (`crate::options`).
const SECTIONS: [(&str, &str, &str); 8] = [
    ("model", "Model", "What the agent runs on."),
    ("safety", "Approvals", "What the agent may do without asking you."),
    ("autonomy", "Autonomy", "How the agent keeps going, and who watches it."),
    ("compaction", "Context", "How a long session’s context is kept within the model’s window. The whole conversation is always kept."),
    ("budgets", "Limits", "How far one run may go before it stops and asks."),
    ("timeouts", "Timeouts", "How long actions and requests may take, and how often they’re tried again."),
    ("collaboration", "Messages", "What other sessions and Coder instances may do."),
    ("instructions", "Instructions", "What the agent is told at the start of every turn."),
];

/// The least the window is tall, so moving between short sections
/// doesn't make it jump.
const MIN_HEIGHT: f32 = 440.0;

const ROLES: [(&str, &str); 4] = [("primary", "This session"), ("supervisor", "The supervisor"), ("reviewer", "The approval reviewer"), ("compaction", "Summarising")];

#[derive(Clone)]
pub enum Msg {
    Section(usize),
    Loaded(Result<Value, String>),
    Toggle(usize, bool),
    Choose(usize, usize),
    Edited,
    Apply,
    Applied(u64, Result<(), String>),
    Revert,
    // The model.
    Provider(usize),
    Model(usize),
    Effort(usize),
    Fetch,
    Fetched(u64, Result<Vec<String>, String>),
    Profile(usize),
    Role(usize),
    UseProfile,
    SaveProfile,
    DeleteProfile,
    Profiles(u64, Result<Vec<ModelProfile>, String>),
}

/// One option as it's being edited.
struct Option_ {
    spec: crate::options::OptionSpec,
    text: Child<TextField>,
    on: bool,
    chosen: usize,
}

pub struct Settings {
    dir: PathBuf,
    /// The window it's over (its `window_key`).
    owner: String,
    session: String,
    client: Handle,
    section: usize,
    config: Option<Value>,
    options: Vec<Option_>,
    said: Option<(String, bool)>,
    busy: bool,
    asked: u64,
    // The model section.
    model: ModelConfig,
    ids: Vec<String>,
    endpoint: Child<TextField>,
    key: Child<TextField>,
    window: Child<TextField>,
    output: Child<TextField>,
    profiles: Vec<ModelProfile>,
    profile: usize,
    role: usize,
    name: Child<TextField>,
}

/// Opens Settings for the session `view` shows, over the window `cx` is in
/// (known as `owner`, so an upgrade brings it back over it), at `section`.
pub fn open(cx: &mut WindowCx, dir: PathBuf, client: Handle, view: &Shown, owner: &str, section: usize) {
    let owner = owner.to_owned();
    let Some(s) = view.session.clone() else { return };
    let ids = view.models.clone();
    let profiles = view.profiles.clone();
    // A panel of its own beside the session's window (never one of its
    // tabs), as tall as the section showing needs.
    let options = WindowOptions { fixed: true, ..WindowOptions::new(format!("Settings — {}", s.title), 680.0, MIN_HEIGHT).dialog() };
    olo_kit::open_window_here(cx, options, move |cx| {
        let field = |cx: &mut Cx<Settings>, name: &str, text: String| {
            let f = text_field(cx, |f| f.name(name.to_owned()).with_text(text));
            cx.subscribe(&f, |_: &mut Settings, e: &TextFieldEvent, cx| {
                if matches!(e, TextFieldEvent::Changed) {
                    cx.send(Msg::Edited);
                }
            });
            f
        };
        let model = s.config.model.clone();
        let endpoint = field(cx, "API root", model.endpoint.as_ref().map(|e| e.base_url.clone()).unwrap_or_default());
        let key = field(cx, "Key", crate::models::key_text(&model));
        let window = field(cx, "Context window", model.context_window.to_string());
        let output = field(cx, "Output tokens", model.max_output_tokens.to_string());
        let name = field(cx, "Saved as", String::new());
        let mut me = Settings {
            dir,
            owner,
            session: s.id.clone(),
            client,
            section: section.min(SECTIONS.len() - 1),
            config: serde_json::to_value(&s.config).ok(),
            options: vec![],
            said: None,
            busy: false,
            asked: 0,
            model,
            ids,
            endpoint,
            key,
            window,
            output,
            profiles,
            profile: 0,
            role: 0,
            name,
        };
        me.load_section(cx);
        me
    });
}

/// The options in section `id`.
fn specs(id: &str) -> Vec<crate::options::OptionSpec> {
    use crate::options::{OptionSpec, section};
    let toggle = |path, label| OptionSpec { path, label, choices: vec!["true", "false"], rows: 1 };
    match id {
        "safety" => {
            let mut v = vec![
                OptionSpec { path: "approval", label: "Actions:", choices: vec!["ask", "auto", "allow"], rows: 1 },
                OptionSpec { path: "approval_guidance", label: "Reviewer’s guidance:", choices: vec![], rows: 5 },
                OptionSpec { path: "sandbox", label: "Files:", choices: vec!["unrestricted", "workspace"], rows: 1 },
                toggle("read_only", "Only read, never change"),
            ];
            v.retain(|_| true);
            v
        }
        "autonomy" => {
            let field = |path, label| OptionSpec { path, label, choices: vec![], rows: 1 };
            vec![
                toggle("keep_going", "Keep going toward the goal by itself"),
                heading("Subagents"),
                toggle("subagents", "Let it hand work to subagents"),
                field("max_subagents", "At once, at most:"),
                field("max_depth", "Levels deep:"),
                heading("Supervisor"),
                toggle("supervisor.enabled", "A second model watches the work"),
                OptionSpec { path: "supervisor.guidance", label: "What it looks for:", choices: vec![], rows: 5 },
                toggle("supervisor.recover_errors", "It may restart work an error stopped"),
                field("supervisor.every_turns", "Looks every (turns):"),
                field("supervisor.max_reviews", "Looks, at most:"),
                field("supervisor.max_interventions", "Steps in, at most:"),
                field("supervisor.token_budget", "Its token budget:"),
                field("supervisor.stall_seconds", "Stalled after (s):"),
            ]
        }
        "collaboration" => vec![toggle("accept_messages", "Take messages from other sessions"), toggle("wake_on_message", "A message may start it when it’s idle")],
        other => section(other),
    }
}

/// A heading over the options after it (no option itself).
fn heading(label: &'static str) -> crate::options::OptionSpec {
    crate::options::OptionSpec { path: "", label, choices: vec![], rows: 0 }
}

/// How a choice reads.
fn choice_label(path: &str, value: &str) -> &'static str {
    match (path, value) {
        ("approval", "ask") => "Ask me first",
        ("approval", "auto") => "A reviewer decides, asks me when unsure",
        ("approval", "allow") => "Allow everything",
        ("sandbox", "workspace") => "Only the workspace folder",
        ("sandbox", "unrestricted") => "Anywhere",
        _ => "",
    }
}

impl Settings {
    fn id(&self) -> &'static str {
        SECTIONS[self.section].0
    }

    fn load_section(&mut self, cx: &mut Cx<Self>) {
        let Some(config) = &self.config else { return };
        let config = config.clone();
        self.options = specs(self.id())
            .into_iter()
            .map(|spec| {
                let value = crate::options::value(&config, spec.path);
                let rows = spec.rows;
                let name = spec.label.trim_end_matches(':').to_owned();
                let text = text_field(cx, |f| {
                    let f = f.name(name).with_text(value.clone());
                    if rows > 1 { f.multiline(false) } else { f }
                });
                cx.subscribe(&text, |_: &mut Settings, e: &TextFieldEvent, cx| {
                    if matches!(e, TextFieldEvent::Changed) {
                        cx.send(Msg::Edited);
                    }
                });
                if rows > 1 {
                    // Read from its beginning.
                    text.update(cx, |f, cx| f.set_selection(0..0, cx));
                }
                let chosen = spec.choices.iter().position(|c| *c == value).unwrap_or(0);
                Option_ { on: value == "true", chosen, text, spec }
            })
            .collect();
        self.said = None;
    }

    fn values(&self, cx: &Cx<Self>) -> (Vec<String>, Vec<String>) {
        self.options
            .iter()
            .filter(|o| !o.spec.path.is_empty())
            .map(|o| {
                let v = if o.spec.choices == ["true", "false"] {
                    o.on.to_string()
                } else if !o.spec.choices.is_empty() {
                    o.spec.choices[o.chosen].to_owned()
                } else {
                    o.text.read(cx).text().to_owned()
                };
                (o.spec.path.to_owned(), v)
            })
            .unzip()
    }

    /// What Apply would change, or why it can't.
    fn changes(&self, cx: &Cx<Self>) -> Result<std::collections::BTreeMap<String, Value>, String> {
        let Some(config) = &self.config else { return Err("Loading…".into()) };
        let (paths, values) = self.values(cx);
        crate::options::validated_changes(config, &paths, &values).map_err(|e| friendly(&e.to_string()))
    }

    fn configure(&mut self, values: Value, cx: &mut Cx<Self>) {
        self.asked += 1;
        let asked = self.asked;
        let (dir, session) = (self.dir.clone(), self.session.clone());
        self.busy = true;
        self.said = Some(("Saving…".into(), false));
        cx.work(
            move || {
                let command: Command = serde_json::from_value(json!({"op":"configure_fields","session":session,"values":values})).map_err(|e| e.to_string())?;
                rpc::call(&dir, &command).map(|_| ()).map_err(|e| friendly(&format!("{e:#}")))
            },
            move |r| Msg::Applied(asked, r),
        );
    }

    fn reload(&mut self, cx: &mut Cx<Self>) {
        let (dir, session) = (self.dir.clone(), self.session.clone());
        cx.work(move || rpc::call(&dir, &Command::Get { session }).map_err(|e| format!("{e:#}")), Msg::Loaded);
        let _ = self.client.tx.send(Action::Refresh);
    }

    /// The model as the fields say.
    fn draft(&self, cx: &Cx<Self>) -> Result<ModelConfig, String> {
        let mut model = self.model.clone();
        model.context_window = self.window.read(cx).text().trim().parse().map_err(|_| "The context window is a number of tokens.".to_owned())?;
        model.max_output_tokens = self.output.read(cx).text().trim().parse().map_err(|_| "Output tokens is a number.".to_owned())?;
        if model.provider == Provider::Openai {
            let known = olo_coder_core::providers::of(&model).is_some();
            let e = model.endpoint.get_or_insert_with(Default::default);
            if !known {
                e.base_url = self.endpoint.read(cx).text().trim().to_owned();
            }
            crate::models::set_key(&mut model, self.key.read(cx).text());
        }
        model.validate().map_err(|e| friendly(&e.to_string()))?;
        Ok(model)
    }

    /// The models to choose from: those fetched, else those the service is
    /// known to have, with the one chosen always among them.
    fn model_ids(&self) -> Vec<String> {
        let mut ids = if self.ids.is_empty() { crate::models::known_ids(&self.model) } else { self.ids.clone() };
        if !ids.contains(&self.model.model) {
            ids.insert(0, self.model.model.clone());
        }
        ids
    }

    /// The model's context window and output allowance, in their fields.
    fn show_limits(&mut self, cx: &mut Cx<Self>) {
        let window = self.model.context_window.to_string();
        let output = self.model.max_output_tokens.to_string();
        self.window.update(cx, |f, cx| f.set_text(window, cx));
        self.output.update(cx, |f, cx| f.set_text(output, cx));
    }

    fn model_section(&self, cx: &mut Cx<Self>) -> Element {
        let known = olo_coder_core::providers::of(&self.model);
        let custom = self.model.provider == Provider::Openai && known.is_none();
        let editable = !self.busy;
        let divider = 140.0;
        let ids = self.model_ids();
        let at = ids.iter().position(|i| *i == self.model.model).unwrap_or(0);
        let services = crate::models::services("Codex, through Token Pool", "An OpenAI-compatible endpoint");
        let mut col = div().flex().flex_col().gap(px(m::SPACING)).child(
            MenuField::new("provider", services.into_iter().map(Into::into).collect(), crate::models::service(&self.model))
                .label("Runs on:")
                .divider(divider)
                .enabled(editable)
                .on_change(cx.msg_with(|i: &usize| Msg::Provider(*i))),
        );
        if custom {
            col = col.child(TextControl::new(cx, &self.endpoint).label("API root:").divider(divider));
        }
        if self.model.provider == Provider::Openai {
            let hint = match known {
                Some(k) => format!("Your {} key: its field in Passwords (op://vault/item/field), or the environment variable the service reads it from. Make one at {}.", k.name, k.keys_page),
                None => "The key’s field in Passwords (op://vault/item/field), or the environment variable the service reads it from. Empty for none.".into(),
            };
            col = col.child(TextControl::new(cx, &self.key).label("Key:").divider(divider)).child(div().pl(px(divider)).child(StringView::new(hint).wrap(true).dim(true).small(true)));
        }
        col = col
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(m::TIGHT))
                    .child(
                        div().flex_1().min_w_0().child(
                            MenuField::new("model", ids.iter().map(|id| crate::models::model_name(&self.model, id).into()).collect(), at)
                                .label("Model:")
                                .divider(divider)
                                .enabled(editable)
                                .on_change(cx.msg_with(|i: &usize| Msg::Model(*i))),
                        ),
                    )
                    .child(Button::new("Fetch List").enabled(editable).on_click(cx.msg(Msg::Fetch))),
            )
            .child(
                MenuField::new("effort", crate::models::EFFORTS.iter().map(|e| (*e).into()).collect(), crate::models::effort(&self.model.effort))
                    .label("Reasoning:")
                    .divider(divider)
                    .enabled(editable)
                    .on_change(cx.msg_with(|i: &usize| Msg::Effort(*i))),
            )
            .child(TextControl::new(cx, &self.window).label("Context window:").divider(divider))
            .child(TextControl::new(cx, &self.output).label("Output tokens:").divider(divider))
            .child(div().flex().justify_end().gap(px(m::SPACING)).child(Button::new("Use for This Session").default(true).enabled(editable).on_click(cx.msg(Msg::UseProfile))));
        // Saved models: made once, used for any role.
        let mut names: Vec<olo_kit::SharedString> = vec!["Not saved".into()];
        names.extend(self.profiles.iter().map(|p| p.name.clone().into()));
        col = col
            .child(olo_kit::widgets::separator())
            .child(StringView::new("Saved models").bold(true))
            .child(StringView::new("Save a model once, then use it for this session, its supervisor, the approval reviewer or summarising, in any session.").wrap(true).dim(true))
            .child(MenuField::new("profiles", names, self.profile).label("Saved model:").divider(divider).enabled(editable).on_change(cx.msg_with(|i: &usize| Msg::Profile(*i))))
            .child(TextControl::new(cx, &self.name).label("Save as:").divider(divider))
            .child(
                MenuField::new("role", ROLES.iter().map(|(_, l)| (*l).into()).collect(), self.role)
                    .label("Use for:")
                    .divider(divider)
                    .enabled(editable)
                    .on_change(cx.msg_with(|i: &usize| Msg::Role(*i))),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(m::SPACING))
                    .child(Button::new("Delete").enabled(editable && self.profile > 0).on_click(cx.msg(Msg::DeleteProfile)))
                    .child(Button::new("Save").enabled(editable && !self.name.read(cx).text().trim().is_empty()).on_click(cx.msg(Msg::SaveProfile))),
            );
        col.into_any_element()
    }

    fn options_section(&self, cx: &mut Cx<Self>) -> Element {
        let divider = 170.0;
        let lh = f32::from(TextField::line_height_for(cx.scale_factor(), style::PLAIN_SIZE));
        let mut col = div().flex().flex_col().gap(px(m::SPACING));
        for (i, o) in self.options.iter().enumerate() {
            let s = &o.spec;
            if s.path.is_empty() {
                col = col.child(div().pt(px(m::SPACING)).child(StringView::new(s.label).bold(true)));
            } else if s.choices == ["true", "false"] {
                // Under the controls, as a form's checkboxes are.
                let label = s.label.trim_end_matches(':').to_owned();
                col = col.child(div().pl(px(divider)).child(CheckBox::new(label, o.on).enabled(!self.busy).on_toggle(cx.msg_with(move |on: &bool| Msg::Toggle(i, *on)))));
            } else if !s.choices.is_empty() {
                let labels = s.choices.iter().map(|c| match choice_label(s.path, c) {
                    "" => (*c).to_owned().into(),
                    l => l.into(),
                });
                col = col.child(
                    MenuField::new(format!("o-{i}"), labels.collect(), o.chosen).label(s.label).divider(divider).enabled(!self.busy).on_change(cx.msg_with(move |at: &usize| Msg::Choose(i, *at))),
                );
            } else if s.rows > 1 {
                let focused = o.text.read(cx).is_focused();
                let well = canvas(move |g, b| {
                    let p = theme::palette();
                    style::fill_rect(g, style::inset(g, b, 2.0), style::c(p.document_background));
                    style::sunken(g, b, &p, focused);
                })
                .paint_key(style::key(focused))
                .absolute()
                .inset_0();
                let h = s.rows.min(8) as f32 * lh + 2.0 * (2.0 + m::TIGHT);
                col = col.child(
                    div()
                        .flex()
                        .gap(px(m::LABEL_GAP))
                        .child(div().w(px(divider - m::LABEL_GAP)).flex_shrink_0().child(StringView::new(s.label)))
                        .child(div().flex_1().min_w_0().relative().h(px(h)).child(well).child(div().absolute().inset(px(2.0 + m::TIGHT)).child(&o.text))),
                );
            } else {
                col = col.child(TextControl::new(cx, &o.text).label(s.label).divider(divider));
            }
        }
        let changes = self.changes(cx);
        let changed = changes.as_ref().is_ok_and(|c| !c.is_empty());
        if let Err(e) = &changes {
            if !self.busy {
                col = col.child(StringView::new(e.clone()).wrap(true).color(style::c(theme::palette().failure)));
            }
        }
        col.child(
            div()
                .flex()
                .justify_end()
                .gap(px(m::SPACING))
                .child(Button::new("Revert").enabled(changed && !self.busy).on_click(cx.msg(Msg::Revert)))
                .child(Button::new("Apply").default(true).enabled(changed && !self.busy).on_click(cx.msg(Msg::Apply))),
        )
        .into_any_element()
    }
}

/// What the service said, as the person would put it.
fn friendly(e: &str) -> String {
    let e = e.trim_start_matches("Error: ");
    if e.contains("idle") && (e.contains("running") || e.contains("busy")) {
        return "The agent is working. This applies once it stops: Stop it, or wait.".into();
    }
    if e.contains("invalid digit") || e.contains("expected") && e.contains("integer") {
        return "That needs a whole number.".into();
    }
    let mut c = e.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

impl View for Settings {
    type Message = Msg;

    /// Back over its window after an upgrade, at the same section (what
    /// was being edited and not applied is let go: the session's settings
    /// are read again).
    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        Some(json!({"form":"settings","owner":self.owner,"section":self.section}).to_string())
    }

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Section(i) => {
                self.section = i.min(SECTIONS.len() - 1);
                self.load_section(cx);
            }
            Msg::Loaded(Ok(session)) => {
                self.config = Some(session["config"].clone());
                if let Ok(model) = serde_json::from_value::<ModelConfig>(session["config"]["model"].clone()) {
                    self.model = model;
                }
                self.load_section(cx);
            }
            Msg::Loaded(Err(e)) => self.said = Some((e, true)),
            Msg::Toggle(i, on) => {
                if let Some(o) = self.options.get_mut(i) {
                    o.on = on;
                }
            }
            Msg::Choose(i, at) => {
                if let Some(o) = self.options.get_mut(i) {
                    o.chosen = at;
                }
            }
            Msg::Edited => {
                if self.said.as_ref().is_some_and(|(_, bad)| *bad) {
                    self.said = None;
                }
            }
            Msg::Apply => match self.changes(cx) {
                Ok(changes) if !changes.is_empty() => self.configure(json!(changes), cx),
                Ok(_) => {}
                Err(e) => self.said = Some((e, true)),
            },
            Msg::Applied(asked, result) => {
                if asked != self.asked {
                    return;
                }
                self.busy = false;
                match result {
                    Ok(()) => {
                        self.said = Some(("Saved.".into(), false));
                        self.reload(cx);
                    }
                    Err(e) => self.said = Some((e, true)),
                }
            }
            Msg::Revert => self.load_section(cx),
            Msg::Provider(i) => {
                if i != crate::models::service(&self.model) {
                    self.model = crate::models::service_model(i);
                    self.ids.clear();
                    let key = crate::models::key_text(&self.model);
                    let root = self.model.endpoint.as_ref().map(|e| e.base_url.clone()).unwrap_or_default();
                    self.key.update(cx, |f, cx| f.set_text(key, cx));
                    self.endpoint.update(cx, |f, cx| f.set_text(root, cx));
                    self.show_limits(cx);
                }
            }
            Msg::Model(i) => {
                if let Some(id) = self.model_ids().get(i) {
                    crate::models::choose(&mut self.model, id);
                    self.show_limits(cx);
                }
            }
            Msg::Effort(i) => self.model.effort = crate::models::effort_value(i),
            Msg::Fetch => match self.draft(cx) {
                Ok(model) => {
                    self.asked += 1;
                    let asked = self.asked;
                    let dir = self.dir.clone();
                    self.busy = true;
                    self.said = Some(("Asking for the list of models…".into(), false));
                    cx.work(
                        move || {
                            let profile = ModelProfile { id: olo_coder_core::id(), name: "draft".into(), model };
                            let command = Command::ModelProfiles { action: "discover".into(), profile: Some(profile), id: None, session: None, role: String::new() };
                            rpc::call(&dir, &command).map_err(|e| format!("{e:#}")).and_then(|v| serde_json::from_value(v["models"].clone()).map_err(|e| e.to_string()))
                        },
                        move |r| Msg::Fetched(asked, r),
                    );
                }
                Err(e) => self.said = Some((e, true)),
            },
            Msg::Fetched(asked, result) => {
                if asked != self.asked {
                    return;
                }
                self.busy = false;
                match result {
                    Ok(ids) => {
                        self.said = Some((format!("{} models.", ids.len()), false));
                        if !ids.contains(&self.model.model) {
                            if let Some(first) = ids.first() {
                                crate::models::choose(&mut self.model, first);
                                self.show_limits(cx);
                            }
                        }
                        self.ids = ids;
                    }
                    Err(e) => self.said = Some((friendly(&e), true)),
                }
            }
            Msg::Profile(i) => {
                self.profile = i.min(self.profiles.len());
                if let Some(p) = self.profile.checked_sub(1).and_then(|i| self.profiles.get(i)).cloned() {
                    self.model = p.model.clone();
                    let (window, output, endpoint, key, name) = (
                        p.model.context_window.to_string(),
                        p.model.max_output_tokens.to_string(),
                        p.model.endpoint.as_ref().map(|e| e.base_url.clone()).unwrap_or_default(),
                        crate::models::key_text(&p.model),
                        p.name.clone(),
                    );
                    self.window.update(cx, |f, cx| f.set_text(window, cx));
                    self.output.update(cx, |f, cx| f.set_text(output, cx));
                    self.endpoint.update(cx, |f, cx| f.set_text(endpoint, cx));
                    self.key.update(cx, |f, cx| f.set_text(key, cx));
                    self.name.update(cx, |f, cx| f.set_text(name, cx));
                }
            }
            Msg::Role(i) => self.role = i.min(ROLES.len() - 1),
            Msg::UseProfile => match self.draft(cx) {
                Ok(model) => {
                    // A saved model is applied for its role; the fields as
                    // they are go to this session.
                    match (self.profile.checked_sub(1).and_then(|i| self.profiles.get(i)), self.role) {
                        (Some(p), role) if role > 0 || p.model == model => {
                            let (dir, id, session, role) = (self.dir.clone(), p.id.clone(), self.session.clone(), ROLES[role].0.to_owned());
                            self.asked += 1;
                            let asked = self.asked;
                            self.busy = true;
                            cx.work(
                                move || {
                                    rpc::call(&dir, &Command::ModelProfiles { action: "apply".into(), profile: None, id: Some(id), session: Some(session), role })
                                        .map(|_| ())
                                        .map_err(|e| friendly(&format!("{e:#}")))
                                },
                                move |r| Msg::Applied(asked, r),
                            );
                        }
                        _ => self.configure(json!({ "model": model }), cx),
                    }
                }
                Err(e) => self.said = Some((e, true)),
            },
            Msg::SaveProfile | Msg::DeleteProfile => {
                let remove = matches!(message, Msg::DeleteProfile);
                let existing = self.profile.checked_sub(1).and_then(|i| self.profiles.get(i)).cloned();
                let profile = if remove {
                    None
                } else {
                    match self.draft(cx) {
                        Ok(model) => {
                            let name = self.name.read(cx).text().trim().to_owned();
                            // The same name saves over it; another is a new one.
                            let id = existing.as_ref().filter(|p| p.name == name).map(|p| p.id.clone()).unwrap_or_else(olo_coder_core::id);
                            Some(ModelProfile { id, name, model })
                        }
                        Err(e) => {
                            self.said = Some((e, true));
                            cx.invalidate();
                            return;
                        }
                    }
                };
                let id = self.profile.checked_sub(1).and_then(|i| self.profiles.get(i)).map(|p| p.id.clone());
                let dir = self.dir.clone();
                self.asked += 1;
                let asked = self.asked;
                self.busy = true;
                cx.work(
                    move || {
                        let command = Command::ModelProfiles { action: if remove { "remove" } else { "save" }.into(), profile, id, session: None, role: String::new() };
                        rpc::call(&dir, &command).map_err(|e| friendly(&format!("{e:#}"))).and_then(|v| serde_json::from_value(v["profiles"].clone()).map_err(|e| e.to_string()))
                    },
                    move |r| Msg::Profiles(asked, r),
                );
            }
            Msg::Profiles(asked, result) => {
                if asked != self.asked {
                    return;
                }
                self.busy = false;
                match result {
                    Ok(profiles) => {
                        let name = self.name.read(cx).text().trim().to_owned();
                        self.profiles = profiles;
                        self.profile = self.profiles.iter().position(|p| p.name == name).map_or(0, |i| i + 1);
                        self.said = Some(("Saved models updated.".into(), false));
                        let _ = self.client.tx.send(Action::Profiles);
                    }
                    Err(e) => self.said = Some((e, true)),
                }
            }
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let rows: Vec<Row> = SECTIONS.iter().map(|(_, title, _)| Row::new("", *title)).collect();
        let sidebar = Sidebar::new("sections", rows, self.section).on_choose(cx.msg_with(|i: &usize| Msg::Section(*i))).build(cx);
        let (_, title, about) = SECTIONS[self.section];
        let content = if self.id() == "model" { self.model_section(cx) } else { self.options_section(cx) };
        let mut pane = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .p(px(m::WINDOW_INSET))
            .child(StringView::new(title).bold(true))
            .child(StringView::new(about).wrap(true).dim(true))
            .child(content);
        if let Some((said, bad)) = &self.said {
            let view = StringView::new(said.clone()).wrap(true);
            pane = pane.child(if *bad { view.color(style::c(theme::palette().failure)) } else { view.dim(true) });
        }
        // As tall as the section needs (the window follows it); the
        // sidebar runs its height.
        div().w_full().min_h(px(MIN_HEIGHT)).flex().child(div().w(px(150.0)).flex_shrink_0().child(sidebar)).child(pane).into_any_element()
    }
}
