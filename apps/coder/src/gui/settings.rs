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
use crate::models::{self, KeyInput, KeyState, Listed, ROLES};
use crate::options::{SECTIONS, choice_label, section as specs, unit};
use olo_coder_core::{Command, ModelConfig, Provider, profiles::ModelProfile, rpc};
use olo_kit::{
    Child, Cx, Element, IntoElement as _, ParentElement as _, SharedString, Styled as _, View, WindowCx, WindowOptions, canvas, div, metrics as m, px, style, theme,
    widgets::{Button, CheckBox, LinkButton, MenuField, Sidebar, StringView, TextControl, TextField, TextFieldEvent, sidebar::Row, text_field},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

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
    /// All that was sent with it (the reviewer that follows it).
    values: BTreeMap<String, Value>,
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
            self.options.iter().filter(|o| !o.spec.path.is_empty() && o.spec.choices.is_empty()).map(|o| (o.spec.path.to_owned(), o.text.read(cx).text().trim().to_owned())).unzip();
        crate::options::validated_changes(&self.config, &paths, &values).map_err(|e| models::friendly(&e.to_string(), true))
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
                rpc::call(&dir, &command).map_err(|e| models::friendly(&format!("{e:#}"), true))
            },
            move |r| Msg::Saved(asked, r),
        );
        asked
    }

    /// Applies `model` to the session, or to new sessions if this
    /// conversation can't move to its service.
    fn save_model(&mut self, model: ModelConfig, cx: &mut Cx<Self>) {
        self.save_model_with(model, BTreeMap::new(), cx);
    }

    /// `save_model`, with other `values` that go with it.
    fn save_model_with(&mut self, model: ModelConfig, mut values: BTreeMap<String, Value>, cx: &mut Cx<Self>) {
        values.insert("model".to_owned(), json!(model));
        let asked = self.save(values.clone(), false, cx);
        self.moving = Some(Moving { asked, model, values });
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
        model.validate().map_err(|e| models::friendly(&e.to_string(), true))?;
        Ok(model)
    }

    /// The session's model as it is (not the one shown, if that's for new
    /// sessions only).
    fn session_model(&self) -> ModelConfig {
        serde_json::from_value(self.config["model"].clone()).unwrap_or_else(|_| self.model.clone())
    }

    /// What role `r`'s menu offers, and which it shows.
    fn role_menu(&self, r: usize) -> (Vec<SharedString>, usize) {
        let (labels, at) = models::role_menu(&self.config, &self.session_model(), &self.listed, r);
        (labels.into_iter().map(Into::into).collect(), at)
    }

    /// What choosing item `k` of role `r`'s menu sets it to (None: nothing).
    fn role_value(&self, r: usize, k: usize) -> Option<Value> {
        models::role_value(&self.session_model(), &self.listed, r, k)
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
            col = col.child(
                MenuField::new(format!("role-{r}"), labels, at)
                    .label(format!("{label}:"))
                    .divider(DIVIDER)
                    .width(POPUP)
                    .enabled(!self.elsewhere)
                    .on_change(cx.msg_with(move |k: &usize| Msg::Role(r, *k))),
            );
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
                        let Moving { model, values, .. } = moving.unwrap();
                        self.elsewhere = true;
                        self.model = model;
                        self.save(values, true, cx);
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
                    // The reviewer, if it was the agent's own model, goes with it.
                    let follow = models::following(&self.config, &self.session_model(), &model).map(|r| ("reviewer".to_owned(), r));
                    self.model = model.clone();
                    self.listed = models::known(&model);
                    self.key_said = None;
                    self.show_model(cx);
                    self.list(cx);
                    self.save_model_with(model, follow.into_iter().collect(), cx);
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
                        let e = models::friendly(&e, true);
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
        assert_eq!(models::friendly("Error: stop the running turn before this operation", true), "The agent is working: its model changes once it stops.");
        assert_eq!(
            models::friendly("Fireworks AI refused the key (HTTP 401): The API key you provided is invalid. Paste another in Settings ▸ Model. Get one at https://example.com/keys.", true),
            "Fireworks AI refused the key (HTTP 401): The API key you provided is invalid. Paste another, or Get a Key."
        );
    }
}
