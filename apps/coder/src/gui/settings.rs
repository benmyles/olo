//! Settings: everything about the session's agent in one window, by
//! section, as Preferences is. A change applies as it's made (a choice at
//! once, typing once it pauses or Return), to this session and, with "Also
//! for new sessions" (on unless turned off), to the sessions started after
//! it; Revert puts the section back as it was when the window opened.
//!
//! The model section is what the agent runs on: the service (the token
//! pool, a known service, or any OpenAI-compatible one), its API key
//! (pasted once, kept in Passwords, and checked by asking the service for
//! its models), the model by name, how hard it thinks, and which models
//! the agent's other jobs use. A conversation stays on the service it
//! began on: choosing another sets it for new sessions and offers one.
//! Changes go to the session through the same API as the terminal's.
use crate::client::{Action, Handle, View as Shown};
use crate::models::{self, KeyInput, KeyState, Listed};
use olo_coder_core::{Command, ModelConfig, Provider, profiles::ModelProfile, rpc};
use olo_kit::{
    Child, Cx, Element, IntoElement as _, ParentElement as _, SharedString, Styled as _, View, WindowCx, WindowOptions, canvas, div, metrics as m, px, style, theme,
    widgets::{Button, CheckBox, LinkButton, MenuField, Sidebar, StringView, TextControl, TextField, TextFieldEvent, sidebar::Row, text_field},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

/// The sections: their ids, titles and what they're for.
const SECTIONS: [(&str, &str, &str); 8] = [
    ("model", "Model", "What the agent runs on, and the models its other jobs use."),
    ("safety", "Approvals", "What the agent may do without asking you."),
    ("autonomy", "Autonomy", "How the agent keeps going, and who watches it."),
    ("compaction", "Context", "How a long conversation is kept within the model’s window. The whole conversation is always kept."),
    ("budgets", "Limits", "How far one run may go before it stops and asks."),
    ("timeouts", "Timeouts", "How long actions and requests may take, and how often they’re tried again."),
    ("collaboration", "Messages", "What other sessions and Coder instances may do."),
    ("instructions", "Instructions", "What the agent is told at the start of every turn."),
];

/// The least the window is tall, so moving between short sections
/// doesn't make it jump.
const MIN_HEIGHT: f32 = 460.0;
/// Where every section's controls begin.
const DIVIDER: f32 = 150.0;
/// How wide the model section's pop-ups are, their labels included (one
/// column, however long their choices' names).
const POPUP: f32 = DIVIDER + 270.0;
/// How long typing pauses before what's typed applies.
const PAUSE: Duration = Duration::from_millis(700);

/// The agent's other jobs a model does: their labels and settings.
const ROLES: [(&str, &str); 3] = [("Supervisor:", "supervisor.model"), ("Approval reviewer:", "reviewer"), ("Summarising:", "compaction.model")];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tone {
    Plain,
    Good,
    Bad,
}

#[derive(Clone)]
pub enum Msg {
    Section(usize),
    Toggle(usize, bool),
    Choose(usize, usize),
    /// An option's text changed: it applies once typing pauses.
    Typed,
    Commit(u64),
    Saved(u64, Result<Value, String>),
    ForNew(bool),
    Revert,
    // The model.
    Service(usize),
    Model(usize),
    Effort(usize),
    Role(usize, usize),
    KeyTyped,
    KeyEntered,
    KeyCommit(u64),
    KeyKept(Result<(), String>),
    /// The address, model id or a limit changed (a custom service's).
    ModelTyped,
    ModelCommit(u64),
    Listed(u64, Result<Vec<Listed>, String>),
    GetKey,
    NewSession,
}

/// One option as it's being edited.
struct Option_ {
    spec: crate::options::OptionSpec,
    text: Child<TextField>,
    on: bool,
    chosen: usize,
}

/// A model change waiting on the service's answer: what's done if the
/// conversation can't move to its service.
struct Moving {
    asked: u64,
    model: ModelConfig,
}

pub struct Settings {
    dir: PathBuf,
    /// The window it's over (its `window_key`).
    owner: String,
    session: String,
    client: Handle,
    section: usize,
    /// The session's settings as last read, and as they were when the
    /// window opened (what Revert goes back to).
    config: Value,
    opened: Value,
    options: Vec<Option_>,
    said: Option<(String, Tone)>,
    /// The last change sent, and the last edit made to options, the key
    /// and a custom service's fields (a pause's timer applies only the
    /// latest of each).
    asked: u64,
    edits: u64,
    key_edits: u64,
    model_edits: u64,
    /// Changes go to new sessions' settings too.
    for_new: bool,
    moving: Option<Moving>,
    reverting: Option<u64>,
    // The model section.
    /// The model as shown: the session's, or one on a service this
    /// conversation couldn't move to (`elsewhere`).
    model: ModelConfig,
    elsewhere: bool,
    listed: Vec<Listed>,
    listing: Option<u64>,
    /// The service gave its list: its address and key work.
    listed_ok: bool,
    key: Child<TextField>,
    key_said: Option<(String, Tone)>,
    keeping: bool,
    address: Child<TextField>,
    model_id: Child<TextField>,
    window: Child<TextField>,
    output: Child<TextField>,
}

/// Opens Settings for the session `view` shows, over the window `cx` is in
/// (known as `owner`, so an upgrade brings it back over it), at `section`.
pub fn open(cx: &mut WindowCx, dir: PathBuf, client: Handle, view: &Shown, owner: &str, section: usize) {
    let owner = owner.to_owned();
    let Some(s) = view.session.clone() else { return };
    let listed = view.models.clone();
    // A panel of its own beside the session's window (never one of its
    // tabs), as tall as the section showing needs.
    let options = WindowOptions { fixed: true, ..WindowOptions::new(format!("Settings — {}", s.title), 700.0, MIN_HEIGHT).dialog() };
    olo_kit::open_window_here(cx, options, move |cx| {
        let field = |cx: &mut Cx<Settings>, name: &str, hint: &str, msg: fn() -> Msg| {
            let f = text_field(cx, |f| f.name(name.to_owned()).hint(hint.to_owned()));
            cx.subscribe(&f, move |_: &mut Settings, e: &TextFieldEvent, cx| match e {
                TextFieldEvent::Changed => cx.send(msg()),
                TextFieldEvent::Entered => cx.send(Msg::KeyEntered),
                _ => {}
            });
            f
        };
        let key = text_field(cx, |f| f.name("API key").password(true));
        cx.subscribe(&key, |_: &mut Settings, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Changed => cx.send(Msg::KeyTyped),
            TextFieldEvent::Entered => cx.send(Msg::KeyEntered),
            _ => {}
        });
        let address = field(cx, "Address", "https://api.example.com/v1", || Msg::ModelTyped);
        let model_id = field(cx, "Model ID", "Its id, as the service names it", || Msg::ModelTyped);
        let window = field(cx, "Context window", "", || Msg::ModelTyped);
        let output = field(cx, "Reply length", "", || Msg::ModelTyped);
        let config = serde_json::to_value(&s.config).unwrap_or_default();
        let model = s.config.model.clone();
        let mut me = Settings {
            dir,
            owner,
            session: s.id.clone(),
            client,
            section: section.min(SECTIONS.len() - 1),
            opened: config.clone(),
            config,
            options: vec![],
            said: None,
            asked: 0,
            edits: 0,
            key_edits: 0,
            model_edits: 0,
            for_new: true,
            moving: None,
            reverting: None,
            listed: if listed.is_empty() { models::known(&model) } else { listed },
            model,
            elsewhere: false,
            listing: None,
            listed_ok: false,
            key,
            key_said: None,
            keeping: false,
            address,
            model_id,
            window,
            output,
        };
        me.show_model(cx);
        me.load_section(cx);
        me.list(cx);
        me
    });
}

/// The options in section `id`, as this window says them.
fn specs(id: &str) -> Vec<crate::options::OptionSpec> {
    use crate::options::OptionSpec;
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

/// What a number setting's value means, after its field ("64 MB", "500K
/// tokens", "10 minutes", "no limit").
fn unit(path: &str, text: &str) -> String {
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
        _ => format!("{} tokens", models::tokens(n)),
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

impl Settings {
    fn id(&self) -> &'static str {
        SECTIONS[self.section].0
    }

    fn load_section(&mut self, cx: &mut Cx<Self>) {
        let config = self.config.clone();
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
                cx.subscribe(&text, |_: &mut Settings, e: &TextFieldEvent, cx| match e {
                    TextFieldEvent::Changed => cx.send(Msg::Typed),
                    TextFieldEvent::Entered => cx.send(Msg::Commit(u64::MAX)),
                    _ => {}
                });
                if rows > 1 {
                    // Read from its beginning.
                    text.update(cx, |f, cx| f.set_selection(0..0, cx));
                }
                let chosen = spec.choices.iter().position(|c| *c == value).unwrap_or(0);
                Option_ { on: value == "true", chosen, text, spec }
            })
            .collect();
    }

    /// What the section's text fields would change, or why they can't.
    fn typed_changes(&self, cx: &Cx<Self>) -> Result<BTreeMap<String, Value>, String> {
        let (paths, values): (Vec<String>, Vec<String>) =
            self.options.iter().filter(|o| !o.spec.path.is_empty() && o.spec.choices.is_empty()).map(|o| (o.spec.path.to_owned(), o.text.read(cx).text().trim().replace(',', ""))).unzip();
        crate::options::validated_changes(&self.config, &paths, &values).map_err(|e| friendly(&e.to_string()))
    }

    /// Sends `values` to the session (and new sessions' settings, if
    /// they're to follow); `session` "" sends them to new sessions only.
    fn save(&mut self, values: BTreeMap<String, Value>, only_new: bool, cx: &mut Cx<Self>) -> u64 {
        self.asked += 1;
        let asked = self.asked;
        let (dir, session, defaults) = (self.dir.clone(), if only_new { String::new() } else { self.session.clone() }, self.for_new);
        self.said = Some(("Saving…".into(), Tone::Plain));
        cx.work(
            move || {
                let command = Command::ConfigureFields { session, values, defaults };
                rpc::call(&dir, &command).map_err(|e| friendly(&format!("{e:#}")))
            },
            move |r| Msg::Saved(asked, r),
        );
        asked
    }

    /// Applies `model` to the session, or to new sessions if this
    /// conversation can't move to its service.
    fn save_model(&mut self, model: ModelConfig, cx: &mut Cx<Self>) {
        let asked = self.save(BTreeMap::from([("model".to_owned(), json!(model))]), false, cx);
        self.moving = Some(Moving { asked, model });
    }

    /// Asks the service for its models (which checks its address and key).
    fn list(&mut self, cx: &mut Cx<Self>) {
        self.asked += 1;
        let asked = self.asked;
        self.listing = Some(asked);
        let (dir, model) = (self.dir.clone(), self.model.clone());
        cx.work(
            move || {
                let profile = ModelProfile { id: olo_coder_core::id(), name: "draft".into(), model };
                let command = Command::ModelProfiles { action: "discover".into(), profile: Some(profile), id: None, session: None, role: String::new() };
                rpc::call(&dir, &command).map_err(|e| format!("{e:#}")).and_then(|v| serde_json::from_value(v["listed"].clone()).map_err(|e| e.to_string()))
            },
            move |r| Msg::Listed(asked, r),
        );
    }

    /// The model's fields as it is.
    fn show_model(&mut self, cx: &mut Cx<Self>) {
        let model = &self.model;
        let address = model.endpoint.as_ref().map(|e| e.base_url.clone()).unwrap_or_default();
        let (id, window, output) = (model.model.clone(), model.context_window.to_string(), model.max_output_tokens.to_string());
        let key = models::key_text(model);
        let kept = matches!(models::key_state(model), KeyState::Kept(_)) || (self.listed_ok && matches!(models::key_state(model), KeyState::Unset(_)));
        self.address.update(cx, |f, cx| f.set_text(address, cx));
        self.model_id.update(cx, |f, cx| f.set_text(id, cx));
        self.window.update(cx, |f, cx| f.set_text(window, cx));
        self.output.update(cx, |f, cx| f.set_text(output, cx));
        self.key.update(cx, |f, cx| {
            f.clear_secret(cx);
            f.set_text(key, cx);
            f.set_hint(if kept { "Paste a new key to replace it" } else { "Paste your API key" }, cx);
        });
    }

    /// The model as a custom service's fields say, or why it can't be.
    fn typed_model(&self, cx: &Cx<Self>) -> Result<ModelConfig, String> {
        let mut model = self.model.clone();
        if model.provider != Provider::Openai || olo_coder_core::providers::of(&model).is_some() {
            return Ok(model);
        }
        let number = |f: &Child<TextField>, what: &str| f.read(cx).text().trim().replace([',', '_'], "").parse().map_err(|_| format!("The {what} is a number of tokens."));
        model.context_window = number(&self.window, "context window")?;
        model.max_output_tokens = number(&self.output, "reply length")? as u32;
        if let Some(e) = &mut model.endpoint {
            e.base_url = self.address.read(cx).text().trim().to_owned();
        }
        let id = self.model_id.read(cx).text().trim();
        if !id.is_empty() {
            model.model = id.to_owned();
        }
        model.validate().map_err(|e| friendly(&e.to_string()))?;
        Ok(model)
    }

    /// The session's model as it is (not the one shown, if that's for new
    /// sessions only).
    fn session_model(&self) -> ModelConfig {
        serde_json::from_value(self.config["model"].clone()).unwrap_or_else(|_| self.model.clone())
    }

    /// What role `r`'s menu offers, and which it shows.
    fn role_menu(&self, r: usize) -> (Vec<SharedString>, usize) {
        let mut labels: Vec<SharedString> = vec!["Same as the agent".into()];
        labels.extend(self.listed.iter().map(|l| l.name.clone().into()));
        let agent = self.session_model();
        let current: Option<ModelConfig> = self.config.pointer(&crate::options::pointer(ROLES[r].1)).and_then(|v| serde_json::from_value(v.clone()).ok());
        let Some(current) = current else { return (labels, 0) };
        let same_service =
            current.provider == agent.provider && current.endpoint.as_ref().map(|e| e.base_url.trim_end_matches('/')) == agent.endpoint.as_ref().map(|e| e.base_url.trim_end_matches('/'));
        if same_service {
            if r == 1 && current.model == agent.model {
                return (labels, 0);
            }
            if let Some(i) = self.listed.iter().position(|l| l.id == current.model) {
                return (labels, i + 1);
            }
        }
        labels.push(format!("{} on {}", models::model_name(&current, &[], &current.model), models::service_name(&current)).into());
        let at = labels.len() - 1;
        (labels, at)
    }

    /// What choosing item `k` of role `r`'s menu sets it to (None: nothing).
    fn role_value(&self, r: usize, k: usize) -> Option<Value> {
        let agent = self.session_model();
        // The reviewer decides quickly: the least effort the model takes.
        let quick = |mut m: ModelConfig, listed: &[Listed]| {
            if r == 1 {
                let efforts = models::efforts(listed, &m.model);
                if efforts.iter().any(|e| e == "low") {
                    m.effort = "low".into();
                }
            }
            m
        };
        match k {
            0 if r == 1 => Some(json!(quick(agent, &self.listed))),
            0 => Some(Value::Null),
            k if k <= self.listed.len() => {
                let mut m = agent;
                models::choose(&mut m, &self.listed, &self.listed[k - 1].id);
                Some(json!(quick(m, &self.listed)))
            }
            _ => None,
        }
    }

    /// The key's line: where it's kept, or what's wrong with it.
    fn key_line(&self) -> (String, Tone) {
        if let Some(said) = &self.key_said {
            return said.clone();
        }
        match models::key_state(&self.model) {
            KeyState::NotNeeded => (String::new(), Tone::Plain),
            KeyState::Kept(title) => (format!("Kept in Passwords as “{title}”."), Tone::Plain),
            KeyState::Unset(title) if self.listed_ok => (format!("Kept in Passwords as “{title}”."), Tone::Plain),
            KeyState::Unset(_) if self.listing.is_some() => (String::new(), Tone::Plain),
            KeyState::Unset(_) => (format!("Paste your {} API key here. Get a Key makes one.", models::service_name(&self.model)), Tone::Plain),
            KeyState::Reference(r) => (format!("Read from {r} in Passwords."), Tone::Plain),
            KeyState::Variable(v) => (format!("Read from ${v} in the Coder service’s environment."), Tone::Plain),
            KeyState::Missing => ("None: fine for a server that doesn’t ask for one.".into(), Tone::Plain),
        }
    }

    fn model_section(&self, cx: &mut Cx<Self>) -> Element {
        let model = &self.model;
        let known = olo_coder_core::providers::of(model);
        let custom = model.provider == Provider::Openai && known.is_none();
        let tone_color = |t: Tone| match t {
            Tone::Bad => Some(style::c(theme::palette().failure)),
            Tone::Good => Some(style::c(theme::palette().success)),
            Tone::Plain => None,
        };
        let note = |text: String, tone: Tone| {
            let v = StringView::new(text).wrap(true).small(true);
            div().pl(px(DIVIDER)).child(match tone_color(tone) {
                Some(c) => v.color(c),
                None => v.dim(true),
            })
        };
        let services = models::services().into_iter().map(Into::into).collect();
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(MenuField::new("service", services, models::service(model)).label("Service:").divider(DIVIDER).width(POPUP).on_change(cx.msg_with(|i: &usize| Msg::Service(*i))));
        if self.elsewhere {
            let here = models::service_name(&self.session_model());
            col = col.child(
                div()
                    .pl(px(DIVIDER))
                    .flex()
                    .items_center()
                    .gap(px(m::SPACING))
                    .child(div().flex_1().min_w_0().child(StringView::new(format!("New sessions start on {}. This conversation stays on {here}.", models::service_name(model))).wrap(true).small(true)))
                    .child(Button::new("New Session").default(true).on_click(cx.msg(Msg::NewSession))),
            );
        }
        if custom {
            col = col
                .child(TextControl::new(cx, &self.address).label("Address:").divider(DIVIDER).width(POPUP))
                .child(note("Its OpenAI-compatible API root, which ends in /v1 as a rule.".into(), Tone::Plain));
        }
        if model.provider == Provider::Openai {
            // As wide as the pop-ups, Get a Key after it.
            let mut row = div().flex().items_center().gap(px(m::SPACING)).child(TextControl::new(cx, &self.key).label("API key:").divider(DIVIDER).width(POPUP));
            if known.is_some() {
                row = row.child(LinkButton::new("Get a Key").on_click(cx.msg(Msg::GetKey)));
            }
            let (line, tone) = self.key_line();
            col = col.child(row);
            if !line.is_empty() {
                col = col.child(note(line, tone));
            }
        }
        // The model: from the service's list, else typed.
        if custom && self.listed.is_empty() {
            col = col.child(TextControl::new(cx, &self.model_id).label("Model:").divider(DIVIDER).width(POPUP));
        } else {
            let mut listed = self.listed.clone();
            if !listed.iter().any(|l| l.id == model.model) {
                listed.insert(0, Listed { id: model.model.clone(), name: models::model_name(model, &[], &model.model), efforts: vec![], context_window: None });
            }
            let at = listed.iter().position(|l| l.id == model.model).unwrap_or(0);
            col = col.child(
                MenuField::new("model", listed.iter().map(|l| l.name.clone().into()).collect(), at).label("Model:").divider(DIVIDER).width(POPUP).on_change(cx.msg_with(|i: &usize| Msg::Model(*i))),
            );
        }
        if self.listing.is_some() {
            col = col.child(note("Asking the service for its models…".into(), Tone::Plain));
        }
        let efforts = models::efforts(&self.listed, &model.model);
        let effort_at = efforts.iter().position(|e| *e == model.effort).unwrap_or(0);
        col = col.child(
            MenuField::new("effort", efforts.iter().map(|e| models::effort_name(e).into()).collect(), effort_at)
                .label("Reasoning:")
                .divider(DIVIDER)
                .width(POPUP)
                .on_change(cx.msg_with(|i: &usize| Msg::Effort(*i))),
        );
        if custom {
            let limit = |cx: &mut Cx<Self>, field: &Child<TextField>, label: &str| {
                let text = field.read(cx).text().to_owned();
                div()
                    .flex()
                    .items_center()
                    .gap(px(m::SPACING))
                    .child(TextControl::new(cx, field).label(label.to_owned()).divider(DIVIDER).width(DIVIDER + 110.0))
                    .child(StringView::new(unit("model.context_window", &text)).dim(true))
            };
            col = col.child(limit(cx, &self.window, "Context window:")).child(limit(cx, &self.output, "Replies, at most:"));
        } else {
            col = col.child(note(
                format!("Reads up to {} tokens of context and writes up to {} a reply.", models::tokens(model.context_window), models::tokens(model.max_output_tokens as u64)),
                Tone::Plain,
            ));
        }
        // The agent's other jobs.
        col = col
            .child(div().pt(px(m::SPACING)).child(StringView::new("Other jobs").bold(true)))
            .child(StringView::new("Which model does each of the agent’s other jobs. The supervisor works only while it’s on (Autonomy).").wrap(true).dim(true).small(true));
        for (r, (label, _)) in ROLES.iter().enumerate() {
            let (labels, at) = self.role_menu(r);
            col = col
                .child(MenuField::new(format!("role-{r}"), labels, at).label(*label).divider(DIVIDER).width(POPUP).enabled(!self.elsewhere).on_change(cx.msg_with(move |k: &usize| Msg::Role(r, *k))));
        }
        col.into_any_element()
    }

    fn options_section(&self, cx: &mut Cx<Self>) -> Element {
        let lh = f32::from(TextField::line_height_for(cx.scale_factor(), style::PLAIN_SIZE));
        let mut col = div().flex().flex_col().gap(px(m::SPACING));
        for (i, o) in self.options.iter().enumerate() {
            let s = &o.spec;
            if s.path.is_empty() {
                col = col.child(div().pt(px(m::SPACING)).child(StringView::new(s.label).bold(true)));
            } else if s.choices == ["true", "false"] {
                // Under the controls, as a form's checkboxes are.
                let label = s.label.trim_end_matches(':').to_owned();
                col = col.child(div().pl(px(DIVIDER)).child(CheckBox::new(label, o.on).on_toggle(cx.msg_with(move |on: &bool| Msg::Toggle(i, *on)))));
            } else if !s.choices.is_empty() {
                let labels = s.choices.iter().map(|c| match choice_label(s.path, c) {
                    "" => (*c).to_owned().into(),
                    l => l.into(),
                });
                col = col.child(MenuField::new(format!("o-{i}"), labels.collect(), o.chosen).label(s.label).divider(DIVIDER).on_change(cx.msg_with(move |at: &usize| Msg::Choose(i, *at))));
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
                        .child(div().w(px(DIVIDER - m::LABEL_GAP)).flex_shrink_0().child(StringView::new(s.label)))
                        .child(div().flex_1().min_w_0().relative().h(px(h)).child(well).child(div().absolute().inset(px(2.0 + m::TIGHT)).child(&o.text))),
                );
            } else {
                // A number: a short field, and what it means after it.
                let text = o.text.read(cx).text().to_owned();
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(m::SPACING))
                        .child(TextControl::new(cx, &o.text).label(s.label).divider(DIVIDER).width(DIVIDER + 110.0))
                        .child(StringView::new(unit(s.path, &text)).dim(true)),
                );
            }
        }
        col.into_any_element()
    }

    /// What Revert would set: the section's settings as they were when the
    /// window opened, where they've changed since.
    fn revert_changes(&self) -> BTreeMap<String, Value> {
        let paths: Vec<&str> =
            if self.id() == "model" { std::iter::once("model").chain(ROLES.iter().map(|(_, p)| *p)).collect() } else { specs(self.id()).iter().map(|s| s.path).filter(|p| !p.is_empty()).collect() };
        paths
            .into_iter()
            .filter_map(|p| {
                let pointer = crate::options::pointer(p);
                let was = self.opened.pointer(&pointer)?;
                (self.config.pointer(&pointer) != Some(was)).then(|| (p.to_owned(), was.clone()))
            })
            .collect()
    }
}

/// What the service said, as the person would put it here (where the
/// key is pasted and Get a Key is beside it).
fn friendly(e: &str) -> String {
    let e = e.trim_start_matches("Error: ");
    let e = match e.find(" Paste another in Settings ▸ Model.").or_else(|| e.find(" Get one at ")).or_else(|| e.find(": paste it in Settings ▸ Model")) {
        Some(at) if e.contains("key") => format!("{} Paste another, or Get a Key.", &e[..at]),
        _ => e.to_owned(),
    };
    let e = e.as_str();
    if e.contains("stop the running turn") || e.contains("the agent is working") {
        return "The agent is working: its model changes once it stops.".into();
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
    /// was being typed and not applied is let go: the session's settings
    /// are read again).
    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        Some(json!({"form":"settings","owner":self.owner,"section":self.section}).to_string())
    }

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Section(i) => {
                self.section = i.min(SECTIONS.len() - 1);
                self.load_section(cx);
                self.said = None;
            }
            Msg::Toggle(i, on) => {
                if let Some(o) = self.options.get_mut(i) {
                    o.on = on;
                    let path = o.spec.path.to_owned();
                    self.save(BTreeMap::from([(path, json!(on))]), false, cx);
                }
            }
            Msg::Choose(i, at) => {
                if let Some(o) = self.options.get_mut(i) {
                    o.chosen = at;
                    let (path, value) = (o.spec.path.to_owned(), o.spec.choices[at].to_owned());
                    self.save(BTreeMap::from([(path, json!(value))]), false, cx);
                }
            }
            Msg::Typed => {
                self.edits += 1;
                cx.after(PAUSE, Msg::Commit(self.edits));
            }
            Msg::Commit(n) => {
                if n != self.edits && n != u64::MAX {
                    return;
                }
                match self.typed_changes(cx) {
                    Ok(changes) if !changes.is_empty() => {
                        self.save(changes, false, cx);
                    }
                    Ok(_) => {}
                    Err(e) => self.said = Some((e, Tone::Bad)),
                }
            }
            Msg::Saved(asked, result) => {
                let moving = self.moving.take_if(|m| m.asked == asked);
                let reverted = self.reverting.take_if(|r| *r == asked).is_some();
                match result {
                    Ok(v) => {
                        if v.get("config").is_some() {
                            self.config = v["config"].clone();
                            self.elsewhere = false;
                            if moving.is_some() || reverted {
                                self.model = self.session_model();
                            }
                            self.said = Some((if self.for_new { "Saved, for new sessions too." } else { "Saved for this session." }.into(), Tone::Plain));
                        } else {
                            self.said = Some(("Saved for new sessions.".into(), Tone::Plain));
                        }
                        if reverted {
                            self.show_model(cx);
                            self.load_section(cx);
                            self.list(cx);
                        }
                        let _ = self.client.tx.send(Action::Refresh);
                    }
                    // A conversation stays on its service: the model is
                    // new sessions', and one is offered.
                    Err(e) if e.contains("start a new session") && moving.is_some() => {
                        let model = moving.unwrap().model;
                        self.elsewhere = true;
                        self.model = model.clone();
                        self.save(BTreeMap::from([("model".to_owned(), json!(model))]), true, cx);
                    }
                    Err(e) => {
                        if moving.is_some() {
                            // Shown as it is, not as asked.
                            self.model = self.session_model();
                            self.show_model(cx);
                        }
                        self.said = Some((e, Tone::Bad));
                    }
                }
            }
            Msg::ForNew(on) => self.for_new = on,
            Msg::Revert => {
                let changes = self.revert_changes();
                if !changes.is_empty() {
                    let asked = self.save(changes, false, cx);
                    self.reverting = Some(asked);
                }
            }
            Msg::Service(i) => {
                if i != models::service(&self.model) {
                    let model = models::service_model(i, &self.model);
                    self.model = model.clone();
                    self.listed = models::known(&model);
                    self.key_said = None;
                    self.show_model(cx);
                    self.list(cx);
                    self.save_model(model, cx);
                }
            }
            Msg::Model(i) => {
                let mut listed = self.listed.clone();
                if !listed.iter().any(|l| l.id == self.model.model) {
                    listed.insert(0, Listed { id: self.model.model.clone(), name: String::new(), efforts: vec![], context_window: None });
                }
                if let Some(id) = listed.get(i).map(|l| l.id.clone()).filter(|id| *id != self.model.model) {
                    let mut model = self.model.clone();
                    models::choose(&mut model, &self.listed, &id);
                    self.model = model.clone();
                    self.show_model(cx);
                    self.save_model(model, cx);
                }
            }
            Msg::Effort(i) => {
                let efforts = models::efforts(&self.listed, &self.model.model);
                if let Some(e) = efforts.get(i).cloned() {
                    self.model.effort = e.clone();
                    if self.elsewhere {
                        let model = self.model.clone();
                        self.save(BTreeMap::from([("model".to_owned(), json!(model))]), true, cx);
                    } else {
                        self.save(BTreeMap::from([("model.effort".to_owned(), json!(e))]), false, cx);
                    }
                }
            }
            Msg::Role(r, k) => {
                if let Some(value) = self.role_value(r, k) {
                    self.save(BTreeMap::from([(ROLES[r].1.to_owned(), value)]), false, cx);
                }
            }
            Msg::KeyTyped => {
                self.key_edits += 1;
                cx.after(PAUSE, Msg::KeyCommit(self.key_edits));
            }
            Msg::KeyEntered => {
                self.message(Msg::KeyCommit(self.key_edits), cx);
                self.message(Msg::ModelCommit(self.model_edits), cx);
                return;
            }
            Msg::KeyCommit(n) => {
                if n != self.key_edits || self.keeping {
                    return;
                }
                let input = models::key_input(self.key.read(cx).text());
                match input {
                    KeyInput::Empty => {}
                    KeyInput::Secret(secret) => {
                        // Kept in Passwords first: the field is let go once it's there.
                        self.keeping = true;
                        self.key_said = Some(("Keeping it in Passwords…".into(), Tone::Plain));
                        let model = self.model.clone();
                        cx.work(move || models::keep_key(&model, &secret), Msg::KeyKept);
                    }
                    other => {
                        if models::key_text(&self.model) != self.key.read(cx).text().trim() {
                            let mut model = self.model.clone();
                            models::set_key(&mut model, &other);
                            self.model = model.clone();
                            self.key_said = None;
                            self.save_model(model, cx);
                            self.list(cx);
                        }
                    }
                }
            }
            Msg::KeyKept(result) => {
                self.keeping = false;
                match result {
                    Ok(()) => {
                        self.key.update(cx, |f, cx| {
                            f.clear_secret(cx);
                            f.set_hint("Paste a new key to replace it", cx);
                        });
                        let mut model = self.model.clone();
                        models::set_key(&mut model, &KeyInput::Secret(String::new()));
                        self.model = model.clone();
                        self.key_said = Some(("Kept in Passwords. Checking it with the service…".into(), Tone::Plain));
                        self.save_model(model, cx);
                        self.list(cx);
                    }
                    Err(e) => self.key_said = Some((e, Tone::Bad)),
                }
            }
            Msg::ModelTyped => {
                self.model_edits += 1;
                cx.after(PAUSE, Msg::ModelCommit(self.model_edits));
            }
            Msg::ModelCommit(n) => {
                if n != self.model_edits {
                    return;
                }
                match self.typed_model(cx) {
                    Ok(model) if model != self.model => {
                        let moved = model.endpoint.as_ref().map(|e| &e.base_url) != self.model.endpoint.as_ref().map(|e| &e.base_url);
                        self.model = model.clone();
                        self.save_model(model, cx);
                        if moved {
                            self.list(cx);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => self.said = Some((e, Tone::Bad)),
                }
            }
            Msg::Listed(asked, result) => {
                if self.listing != Some(asked) {
                    return;
                }
                self.listing = None;
                let keyed = self.model.provider == Provider::Openai;
                self.listed_ok = result.is_ok();
                match result {
                    Ok(listed) => {
                        if keyed && self.key_said.as_ref().is_some_and(|(s, _)| s.contains("Checking")) {
                            self.key_said = Some((format!("{} took the key. It’s kept in Passwords.", models::service_name(&self.model)), Tone::Good));
                        }
                        self.listed = listed;
                    }
                    // No key yet: the line under the key says to paste one.
                    Err(e) if keyed && e.contains("needs your API key: paste") => {
                        if self.key_said.as_ref().is_some_and(|(s, _)| s.contains("Checking")) {
                            self.key_said = None;
                        }
                    }
                    Err(e) => {
                        let e = friendly(&e);
                        if keyed {
                            // Most often the key: said under it.
                            self.key_said = Some((e, Tone::Bad));
                        } else {
                            self.said = Some((e, Tone::Bad));
                        }
                        if self.listed.is_empty() {
                            self.listed = models::known(&self.model);
                        }
                    }
                }
            }
            Msg::GetKey => {
                if let Some(k) = olo_coder_core::providers::of(&self.model) {
                    let link = k.keys_page.to_owned();
                    std::thread::spawn(move || {
                        let _ = olo_xdg::open::open(&olo_xdg::open::Target::parse(&link));
                    });
                }
            }
            Msg::NewSession => {
                let _ = self.client.tx.send(Action::Submit("/new".into()));
                cx.close_window();
                return;
            }
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let rows: Vec<Row> = SECTIONS.iter().map(|(_, title, _)| Row::new("", *title)).collect();
        let sidebar = Sidebar::new("sections", rows, self.section).on_choose(cx.msg_with(|i: &usize| Msg::Section(*i))).build(cx);
        let (_, title, about) = SECTIONS[self.section];
        let content = if self.id() == "model" { self.model_section(cx) } else { self.options_section(cx) };
        let status = match &self.said {
            Some((said, Tone::Bad)) => StringView::new(said.clone()).wrap(true).color(style::c(theme::palette().failure)),
            Some((said, _)) => StringView::new(said.clone()).wrap(true).dim(true),
            None => StringView::new(""),
        };
        let footer = div()
            .flex()
            .items_center()
            .gap(px(m::SPACING))
            .child(CheckBox::new("Also for new sessions", self.for_new).on_toggle(cx.msg_with(|on: &bool| Msg::ForNew(*on))))
            .child(div().flex_1().min_w_0().child(status))
            .child(Button::new("Revert").enabled(!self.revert_changes().is_empty()).on_click(cx.msg(Msg::Revert)));
        let pane = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .p(px(m::WINDOW_INSET))
            .child(StringView::new(title).bold(true))
            .child(StringView::new(about).wrap(true).dim(true))
            .child(content)
            .child(div().flex_1())
            .child(olo_kit::widgets::separator())
            .child(footer);
        // As tall as the section needs (the window follows it); the
        // sidebar runs its height.
        div().w_full().min_h(px(MIN_HEIGHT)).flex().child(div().w(px(150.0)).flex_shrink_0().child(sidebar)).child(pane).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_option_is_a_setting_and_numbers_say_what_they_mean() {
        let config = json!(olo_coder_core::Config::default());
        for (id, _, _) in SECTIONS {
            for spec in specs(id).iter().filter(|s| !s.path.is_empty()) {
                assert!(config.pointer(&crate::options::pointer(spec.path)).is_some(), "{id}: {}", spec.path);
            }
        }
        for (_, path) in ROLES {
            assert!(config.pointer(&crate::options::pointer(path)).is_some(), "{path}");
        }
        assert_eq!(unit("max_tool_bytes", "67108864"), "64 MB");
        assert_eq!(unit("max_response_bytes", "1572864"), "1.5 MB");
        assert_eq!(unit("model.compact_at", "500000"), "500K tokens");
        assert_eq!(unit("token_budget", "0"), "no limit");
        assert_eq!(unit("command_timeout_secs", "600"), "10 minutes");
        assert_eq!(unit("retries", "1"), "time");
        assert_eq!(friendly("Error: stop the running turn before this operation"), "The agent is working: its model changes once it stops.");
        assert_eq!(
            friendly("Fireworks AI refused the key (HTTP 401): The API key you provided is invalid. Paste another in Settings ▸ Model. Get one at https://example.com/keys."),
            "Fireworks AI refused the key (HTTP 401): The API key you provided is invalid. Paste another, or Get a Key."
        );
    }
}
