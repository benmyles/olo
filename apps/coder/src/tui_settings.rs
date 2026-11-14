//! Settings in the terminal, by keyboard: the window's sections, said the
//! same way (`options::SECTIONS`), and the same Model page (`models`). A
//! change applies as it's made, as in the window: a choice as ← → changes
//! it, what's typed on Return or on moving to another field; Esc closes
//! once it has applied. "Also for new sessions" (on unless turned off)
//! sends a change to the sessions started after this one too.
use crate::{
    client::{Action, Handle, Update, View},
    models::{self, KeyInput, KeyState, Listed, ROLES},
    options::{self, OptionSpec, SECTIONS},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use olo_coder_core::{ModelConfig, Provider};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListState, Paragraph},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(serde::Serialize, serde::Deserialize)]
pub enum Panel {
    /// A form: a subagent's task, a message, a connection.
    Entry { key: String, title: String, description: String, fields: Vec<crate::form::Field>, submission: Box<crate::form::Submission>, at: usize, status: String, busy: bool },
    /// Settings' sections and what else it does, or a connection's actions.
    Menu {
        title: String,
        items: Vec<(String, String)>,
        selected: usize,
        #[serde(default)]
        query: String,
    },
    /// Settings ▸ Model.
    Model(Box<ModelPage>),
    /// Another of Settings' sections.
    Options(Box<OptionsPage>),
}

fn yes() -> bool {
    true
}

/// How a saved change reads.
fn saved(for_new: bool) -> String {
    if for_new { "Saved, for new sessions too.".into() } else { "Saved for this session.".into() }
}

/// Sends `values` to session `session` ("": to new sessions only), and
/// to new sessions' settings too if `for_new`: the request's key, None if
/// the connection has closed.
fn send(client: &Handle, session: &str, values: &BTreeMap<String, Value>, for_new: bool) -> Option<String> {
    let key = olo_coder_core::id();
    let text = format!("/api {}", json!({"op":"configure_fields","session":session,"values":values,"defaults":for_new || session.is_empty()}));
    client.tx.send(Action::SubmitSettings { key: key.clone(), text }).ok().map(|_| key)
}

/// Puts `values` (settings' paths) into `config`.
fn put(config: &mut Value, values: &BTreeMap<String, Value>) {
    for (path, value) in values {
        if let Some(at) = config.pointer_mut(&options::pointer(path)) {
            *at = value.clone();
        }
    }
}

/// The one of `count` choices `delta` from `at`, round.
fn turn(at: usize, count: usize, delta: isize) -> usize {
    if count == 0 { 0 } else { (at as isize + delta).rem_euclid(count as isize) as usize }
}

/// Which way ← → (and Return, Space) go.
fn delta(key: &KeyEvent) -> Option<isize> {
    match key.code {
        KeyCode::Left => Some(-1),
        KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ') => Some(1),
        _ => None,
    }
}

/// Typing into `text`: true if it changed.
fn typed(text: &mut String, key: &KeyEvent, multiline: bool) -> bool {
    match key.code {
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => text.clear(),
        KeyCode::Enter if multiline => text.push('\n'),
        KeyCode::Backspace => {
            text.pop();
        }
        KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => text.push(c),
        _ => return false,
    }
    true
}

/// `text` in lines of at most `width` characters, broken between words
/// where it can be.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let (have, more) = (line.chars().count(), word.chars().count());
            if have > 0 && have + 1 + more > width {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
            while line.chars().count() > width {
                let rest: String = line.chars().skip(width).collect();
                out.push(line.chars().take(width).collect());
                line = rest;
            }
        }
        out.push(line);
    }
    out
}

const PICKED: Style = Style::new().fg(Color::Black).bg(Color::LightYellow);
const DIM: Style = Style::new().fg(Color::DarkGray);
const BAD: Style = Style::new().fg(Color::LightRed);

/// A page's lines, each with the row it shows (None: a note), drawn so
/// the chosen row's last line shows, and what's said under them.
fn page(frame: &mut Frame, area: Rect, title: String, lines: Vec<(Option<usize>, Line<'static>)>, at: usize, said: &str, bad: bool) {
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let said = wrap(said, inner.width as usize);
    let foot = (said.len() as u16).min(inner.height.saturating_sub(1)).min(3);
    let list = Rect { height: inner.height.saturating_sub(foot), ..inner };
    let height = list.height as usize;
    let end = lines.iter().rposition(|(row, _)| *row == Some(at)).map_or(0, |n| n + 1);
    // The chosen row's notes show with it.
    let end = (end..lines.len()).take_while(|&n| lines[n].0.is_none()).last().map_or(end, |n| n + 1);
    let skip = end.saturating_sub(height);
    frame.render_widget(Paragraph::new(lines.into_iter().skip(skip).take(height).map(|(_, l)| l).collect::<Vec<_>>()), list);
    let style = if bad { BAD } else { DIM };
    let foot_area = Rect { y: list.y + list.height, height: foot, ..inner };
    frame.render_widget(Paragraph::new(said.into_iter().map(|l| Line::styled(l, style)).collect::<Vec<_>>()), foot_area);
}

/// A row: its mark, label (padded to `width`) and value, styled.
fn row_line(chosen: bool, label: &str, width: usize, value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::raw(format!("{} {label:width$} ", if chosen { "›" } else { " " }))];
    spans.extend(value);
    let line = Line::from(spans);
    if chosen { line.style(PICKED) } else { line }
}

/// A choice's value, with the arrows that change it.
fn choice(value: &str) -> Vec<Span<'static>> {
    vec![Span::raw(format!("‹ {value} ›"))]
}

/// A note under a row, indented to its values.
fn notes(text: &str, indent: usize, width: usize, style: Style) -> Vec<(Option<usize>, Line<'static>)> {
    if text.is_empty() {
        return vec![];
    }
    wrap(text, width.saturating_sub(indent)).into_iter().map(|l| (None, Line::styled(format!("{}{l}", " ".repeat(indent)), style))).collect()
}

/// One of Settings' sections but the model, its options as the window has
/// them.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct OptionsPage {
    section: String,
    session: String,
    /// The session's settings as last known (a change taken is put in).
    config: Value,
    /// Each option's value as shown (a heading's: "").
    values: Vec<String>,
    /// The chosen row: an option, or (`specs().len()`) "Also for new
    /// sessions".
    at: usize,
    /// The chosen option was typed in since it applied.
    #[serde(default)]
    edited: bool,
    #[serde(default = "yes")]
    for_new: bool,
    said: String,
    #[serde(default)]
    bad: bool,
    /// The change sent and not yet answered.
    #[serde(default)]
    request: Option<(String, BTreeMap<String, Value>)>,
    /// Esc was pressed: it closes once what was sent is taken.
    #[serde(default)]
    closing: bool,
}

impl OptionsPage {
    pub fn new(section: &str, view: &View) -> Option<Self> {
        let s = view.session.as_ref()?;
        let config = json!(s.config);
        let values = options::section(section).iter().map(|o| if o.path.is_empty() { String::new() } else { options::value(&config, o.path) }).collect();
        let mut page = Self { section: section.into(), session: s.id.clone(), config, values, at: 0, edited: false, for_new: true, said: String::new(), bad: false, request: None, closing: false };
        page.at = page.next(usize::MAX, 1);
        Some(page)
    }

    fn specs(&self) -> Vec<OptionSpec> {
        options::section(&self.section)
    }

    /// The next row after `from` (`usize::MAX`: the first) going `delta`,
    /// past headings.
    fn next(&self, from: usize, delta: isize) -> usize {
        let specs = self.specs();
        let count = specs.len() + 1;
        let mut at = if from == usize::MAX { count - 1 } else { from };
        for _ in 0..count {
            at = turn(at, count, delta);
            if at == specs.len() || !specs[at].path.is_empty() {
                return at;
            }
        }
        from
    }

    /// The session's settings read again, unless a change is on its way.
    fn follow(&mut self, view: &View) {
        let Some(s) = view.session.as_ref().filter(|s| s.id == self.session) else { return };
        let config = json!(s.config);
        if self.request.is_some() || self.edited || config == self.config {
            return;
        }
        self.values = self.specs().iter().map(|o| if o.path.is_empty() { String::new() } else { options::value(&config, o.path) }).collect();
        self.config = config;
    }

    /// Sends the chosen option's value if it changed: whether it sent it.
    fn apply(&mut self, client: &Handle) -> bool {
        let specs = self.specs();
        let Some(spec) = specs.get(self.at).filter(|s| !s.path.is_empty()) else { return false };
        self.edited = false;
        match options::validated_changes(&self.config, &[spec.path.to_owned()], &[self.values[self.at].clone()]) {
            Ok(changes) if changes.is_empty() => false,
            Ok(changes) => match send(client, &self.session, &changes, self.for_new) {
                Some(key) => {
                    self.request = Some((key, changes));
                    (self.said, self.bad) = ("Saving…".into(), false);
                    true
                }
                None => {
                    (self.said, self.bad) = ("The connection closed: nothing was saved.".into(), true);
                    false
                }
            },
            Err(e) => {
                self.edited = true;
                (self.said, self.bad) = (models::friendly(&e.to_string(), false), true);
                false
            }
        }
    }

    /// True closes Settings.
    fn update(&mut self, update: &Update) -> bool {
        let Update::SettingsSaved { key, result } = update else { return false };
        if self.request.as_ref().is_none_or(|(k, _)| k != key) {
            return false;
        }
        let (_, changes) = self.request.take().unwrap();
        match result {
            Ok(()) => {
                put(&mut self.config, &changes);
                (self.said, self.bad) = (saved(self.for_new), false);
                self.closing && !self.edited
            }
            Err(e) => {
                (self.said, self.bad) = (models::friendly(e, false), true);
                self.closing = false;
                false
            }
        }
    }

    fn paste(&mut self, text: &str) {
        let specs = self.specs();
        if let Some(spec) = specs.get(self.at).filter(|s| !s.path.is_empty() && s.choices.is_empty()) {
            let text = if spec.rows > 1 { text.to_owned() } else { text.replace(['\n', '\r'], "") };
            self.values[self.at].push_str(&text);
            self.edited = true;
        }
    }

    /// True closes Settings.
    fn key(&mut self, key: KeyEvent, view: &View, client: &Handle) -> bool {
        if self.request.is_some() {
            // Answered in a moment: Esc closes once it is.
            self.closing |= key.code == KeyCode::Esc;
            return false;
        }
        self.follow(view);
        let specs = self.specs();
        let text = specs.get(self.at).is_some_and(|s| s.choices.is_empty());
        match key.code {
            KeyCode::Esc => {
                if self.edited {
                    if self.apply(client) {
                        self.closing = true;
                        return false;
                    }
                    if self.bad {
                        return false;
                    }
                }
                return true;
            }
            KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab => {
                if self.edited {
                    self.apply(client);
                    if self.bad {
                        return false;
                    }
                }
                self.at = self.next(self.at, if matches!(key.code, KeyCode::Up | KeyCode::BackTab) { -1 } else { 1 });
            }
            KeyCode::Enter if text && specs[self.at].rows <= 1 => {
                self.apply(client);
            }
            _ if self.at == specs.len() => {
                if delta(&key).is_some() {
                    self.for_new = !self.for_new;
                }
            }
            _ if !text => {
                if let Some(d) = delta(&key) {
                    let choices = &specs[self.at].choices;
                    let i = choices.iter().position(|c| *c == self.values[self.at]).unwrap_or(0);
                    self.values[self.at] = choices[turn(i, choices.len(), d)].to_owned();
                    self.apply(client);
                }
            }
            _ => {
                if typed(&mut self.values[self.at], &key, specs[self.at].rows > 1) {
                    self.edited = true;
                }
            }
        }
        false
    }

    fn draw(&self, frame: &mut Frame, area: Rect) {
        let (title, about) = SECTIONS.iter().find(|(id, ..)| *id == self.section).map_or(("Settings", ""), |(_, t, a)| (*t, *a));
        let width = area.width.saturating_sub(2) as usize;
        let specs = self.specs();
        // Labels in a column, as wide as the longest (a toggle's is its
        // own line); on their own lines when it's narrow.
        let label = specs.iter().filter(|s| !s.path.is_empty() && s.choices != ["true", "false"]).map(|s| s.label.chars().count()).max().unwrap_or(0).min(24);
        let label = if width >= label + 30 { label } else { 0 };
        let mut lines: Vec<(Option<usize>, Line<'static>)> = notes(about, 0, width, DIM);
        lines.push((None, Line::default()));
        for (i, (spec, value)) in specs.iter().zip(&self.values).enumerate() {
            let chosen = i == self.at;
            if spec.path.is_empty() {
                lines.push((None, Line::default()));
                lines.push((None, Line::styled(spec.label.to_owned(), Style::new().add_modifier(Modifier::BOLD))));
                continue;
            }
            if spec.choices == ["true", "false"] {
                let line = Line::raw(format!("{} [{}] {}", if chosen { "›" } else { " " }, if value == "true" { "x" } else { " " }, spec.label));
                lines.push((Some(i), if chosen { line.style(PICKED) } else { line }));
                continue;
            }
            if !spec.choices.is_empty() {
                let shown = options::choice_label(spec.path, value);
                lines.push((Some(i), row_line(chosen, spec.label, label, choice(if shown.is_empty() { value } else { shown }))));
                continue;
            }
            // Typed: its own lines kept and wrapped under its label, and
            // what a number means after it.
            let room = width.saturating_sub(label + 3).max(8);
            let unit = options::unit(spec.path, value);
            let mut rows = wrap(value, room);
            if label == 0 {
                lines.push((Some(i), row_line(chosen, spec.label, 0, vec![])));
            }
            let first = rows.remove(0);
            let mut value = vec![Span::raw(first)];
            if !unit.is_empty() && rows.is_empty() {
                value.push(Span::styled(format!("  {unit}"), if chosen { PICKED } else { DIM }));
            }
            lines.push((Some(i), row_line(chosen, if label == 0 { "" } else { spec.label }, label, value)));
            for r in rows {
                lines.push((Some(i), row_line(chosen, "", label, vec![Span::raw(r)])));
            }
        }
        lines.push((None, Line::default()));
        let chosen = self.at == specs.len();
        let line = Line::raw(format!("{} [{}] Also for new sessions", if chosen { "›" } else { " " }, if self.for_new { "x" } else { " " }));
        lines.push((Some(specs.len()), if chosen { line.style(PICKED) } else { line }));
        page(frame, area, format!(" {title} · ↑↓ move · ←→ change · Esc done "), lines, self.at, &self.said, self.bad);
    }
}

/// A row of the Model page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    Service,
    Address,
    Key,
    Model,
    Effort,
    Window,
    Output,
    Role(usize),
    ForNew,
}

/// A change to the model sent and not yet answered.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
enum Sent {
    /// The agent's model, and all sent with it (the reviewer that follows
    /// it): what's done if the conversation can't move to its service.
    Model(ModelConfig, BTreeMap<String, Value>),
    Values(BTreeMap<String, Value>),
    /// The model for new sessions only.
    NewOnly,
}

/// Settings ▸ Model, as the window has it: the service, its key (pasted
/// once and kept in Passwords), the model, how hard it thinks, and the
/// models of the agent's other jobs.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ModelPage {
    session: String,
    config: Value,
    /// The model as shown: the session's, or one on a service this
    /// conversation couldn't move to (`elsewhere`), for new sessions.
    model: ModelConfig,
    elsewhere: bool,
    listed: Vec<Listed>,
    #[serde(default)]
    listing: Option<String>,
    /// The service gave its list: its address and key work.
    #[serde(default)]
    listed_ok: bool,
    address: String,
    /// What's pasted for the key (never kept with the rest for an
    /// upgrade).
    #[serde(skip)]
    key: String,
    model_id: String,
    window: String,
    output: String,
    at: usize,
    #[serde(default)]
    edited: bool,
    /// The model list open: what's typed to find one, and the one chosen.
    #[serde(default)]
    picker: Option<(String, usize)>,
    #[serde(default = "yes")]
    for_new: bool,
    said: String,
    #[serde(default)]
    bad: bool,
    #[serde(default)]
    key_said: Option<(String, bool)>,
    #[serde(default)]
    request: Option<(String, Sent)>,
    #[serde(default)]
    keep_request: Option<String>,
    #[serde(default)]
    closing: bool,
}

impl ModelPage {
    pub fn new(view: &View) -> Option<Self> {
        let s = view.session.as_ref()?;
        let model = s.config.model.clone();
        let listed = if view.models.is_empty() { models::known(&model) } else { view.models.clone() };
        let mut page = Self {
            session: s.id.clone(),
            config: json!(s.config),
            model,
            elsewhere: false,
            listed,
            listing: None,
            listed_ok: false,
            address: String::new(),
            key: String::new(),
            model_id: String::new(),
            window: String::new(),
            output: String::new(),
            at: 0,
            edited: false,
            picker: None,
            for_new: true,
            said: String::new(),
            bad: false,
            key_said: None,
            request: None,
            keep_request: None,
            closing: false,
        };
        page.show_model();
        Some(page)
    }

    /// The service is one Coder doesn't know: its address, model and
    /// limits are typed.
    fn custom(&self) -> bool {
        self.model.provider == Provider::Openai && olo_coder_core::providers::of(&self.model).is_none()
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Service];
        if self.custom() {
            rows.push(Row::Address);
        }
        if self.model.provider == Provider::Openai {
            rows.push(Row::Key);
        }
        rows.extend([Row::Model, Row::Effort]);
        if self.custom() {
            rows.extend([Row::Window, Row::Output]);
        }
        rows.extend((0..ROLES.len()).map(Row::Role));
        rows.push(Row::ForNew);
        rows
    }

    fn row(&self) -> Row {
        let rows = self.rows();
        rows[self.at.min(rows.len() - 1)]
    }

    /// The model is typed: a custom service that didn't list its models.
    fn typed_model(&self) -> bool {
        self.custom() && self.listed.is_empty()
    }

    /// The chosen row is typed in.
    fn is_text(&self) -> bool {
        match self.row() {
            Row::Address | Row::Key | Row::Window | Row::Output => true,
            Row::Model => self.typed_model(),
            _ => false,
        }
    }

    /// The chosen row's text, if it's typed.
    fn text(&mut self) -> Option<&mut String> {
        match self.row() {
            Row::Address => Some(&mut self.address),
            Row::Key => Some(&mut self.key),
            Row::Model if self.typed_model() => Some(&mut self.model_id),
            Row::Window => Some(&mut self.window),
            Row::Output => Some(&mut self.output),
            _ => None,
        }
    }

    /// The model's fields as it is.
    fn show_model(&mut self) {
        let m = &self.model;
        self.address = m.endpoint.as_ref().map(|e| e.base_url.clone()).unwrap_or_default();
        self.model_id = m.model.clone();
        self.window = m.context_window.to_string();
        self.output = m.max_output_tokens.to_string();
        self.key.clear();
        self.edited = false;
    }

    /// The session's model (not the one shown, if that's for new
    /// sessions only).
    fn agent(&self) -> ModelConfig {
        models::agent_model(&self.config).unwrap_or_else(|| self.model.clone())
    }

    /// The session's settings read again, unless a change is on its way.
    fn follow(&mut self, view: &View) {
        let Some(s) = view.session.as_ref().filter(|s| s.id == self.session) else { return };
        let config = json!(s.config);
        if self.request.is_some() || self.keep_request.is_some() || self.edited || config == self.config {
            return;
        }
        if !self.elsewhere && s.config.model != self.model {
            self.model = s.config.model.clone();
            self.show_model();
        }
        self.config = config;
    }

    /// Asks the service for its models (which checks its address and key).
    pub fn list(&mut self, client: &Handle) {
        let key = olo_coder_core::id();
        if client.tx.send(Action::ListModels { key: key.clone(), model: Box::new(self.model.clone()) }).is_ok() {
            self.listing = Some(key);
        }
    }

    fn sent(&mut self, key: Option<String>, what: Sent) {
        match key {
            Some(key) => {
                self.request = Some((key, what));
                (self.said, self.bad) = ("Saving…".into(), false);
            }
            None => (self.said, self.bad) = ("The connection closed: nothing was saved.".into(), true),
        }
    }

    /// Applies `model` to the session, or to new sessions if this
    /// conversation can't move to its service.
    fn save_model(&mut self, model: ModelConfig, client: &Handle) {
        self.save_model_with(model, BTreeMap::new(), client);
    }

    /// `save_model`, with other `values` that go with it.
    fn save_model_with(&mut self, model: ModelConfig, mut values: BTreeMap<String, Value>, client: &Handle) {
        values.insert("model".to_owned(), json!(model));
        let key = send(client, &self.session, &values, self.for_new);
        self.sent(key, Sent::Model(model, values));
    }

    fn save(&mut self, values: BTreeMap<String, Value>, client: &Handle) {
        let key = send(client, &self.session, &values, self.for_new);
        self.sent(key, Sent::Values(values));
    }

    /// The model as a custom service's fields say, or why it can't be.
    fn typed(&self) -> Result<ModelConfig, String> {
        let mut model = self.model.clone();
        if !self.custom() {
            return Ok(model);
        }
        let number = |text: &str, what: &str| text.trim().replace([',', '_'], "").parse::<u64>().map_err(|_| format!("The {what} is a number of tokens."));
        model.context_window = number(&self.window, "context window")?;
        model.max_output_tokens = number(&self.output, "reply length")? as u32;
        if let Some(e) = &mut model.endpoint {
            e.base_url = self.address.trim().to_owned();
        }
        if !self.model_id.trim().is_empty() {
            model.model = self.model_id.trim().to_owned();
        }
        model.validate().map_err(|e| models::friendly(&e.to_string(), false))?;
        Ok(model)
    }

    /// Applies what was typed in the chosen row: whether it may move on.
    fn commit(&mut self, client: &Handle) -> bool {
        if !self.edited {
            return true;
        }
        self.edited = false;
        if self.row() == Row::Key {
            match models::key_input(&self.key) {
                KeyInput::Empty => {}
                KeyInput::Secret(secret) => {
                    // Kept in Passwords first; the model follows once it's there.
                    let key = olo_coder_core::id();
                    if client.tx.send(Action::KeepKey { key: key.clone(), model: Box::new(self.model.clone()), secret }).is_ok() {
                        self.keep_request = Some(key);
                        self.key_said = Some(("Keeping it in Passwords…".into(), false));
                    }
                }
                other => {
                    let mut model = self.model.clone();
                    models::set_key(&mut model, &other);
                    self.model = model.clone();
                    self.key.clear();
                    self.key_said = None;
                    self.save_model(model, client);
                    self.list(client);
                }
            }
            return true;
        }
        match self.typed() {
            Ok(model) if model != self.model => {
                let moved = model.endpoint.as_ref().map(|e| &e.base_url) != self.model.endpoint.as_ref().map(|e| &e.base_url);
                self.model = model.clone();
                self.save_model(model, client);
                if moved {
                    self.list(client);
                }
                true
            }
            Ok(_) => true,
            Err(e) => {
                self.edited = true;
                (self.said, self.bad) = (e, true);
                false
            }
        }
    }

    /// The models in the Model row's choice: the service's, and the one
    /// it has if the service doesn't list it.
    fn choices(&self) -> Vec<Listed> {
        let mut listed = self.listed.clone();
        if !listed.iter().any(|l| l.id == self.model.model) {
            listed.insert(0, Listed { id: self.model.model.clone(), name: models::model_name(&self.model, &[], &self.model.model), efforts: vec![], context_window: None });
        }
        listed
    }

    /// The models the open list finds by what's typed (by name or id).
    fn found(&self) -> Vec<Listed> {
        let query = self.picker.as_ref().map(|(q, _)| q.to_lowercase()).unwrap_or_default();
        self.choices().into_iter().filter(|l| l.name.to_lowercase().contains(&query) || l.id.to_lowercase().contains(&query)).collect()
    }

    fn choose(&mut self, id: &str, client: &Handle) {
        if id != self.model.model {
            let mut model = self.model.clone();
            models::choose(&mut model, &self.listed, id);
            self.model = model.clone();
            self.show_model();
            self.save_model(model, client);
        }
    }

    /// The chosen row's choice turned `d`.
    fn change(&mut self, d: isize, client: &Handle) {
        match self.row() {
            Row::Service => {
                let count = models::services().len();
                let model = models::service_model(turn(models::service(&self.model), count, d), &self.model);
                // The reviewer, if it was the agent's own model, goes with it.
                let follow = models::following(&self.config, &self.agent(), &model).map(|r| ("reviewer".to_owned(), r));
                self.model = model.clone();
                self.listed = models::known(&model);
                self.listed_ok = false;
                self.key_said = None;
                self.show_model();
                self.list(client);
                self.save_model_with(model, follow.into_iter().collect(), client);
            }
            Row::Model if !self.typed_model() => {
                let listed = self.choices();
                let at = listed.iter().position(|l| l.id == self.model.model).unwrap_or(0);
                let id = listed[turn(at, listed.len(), d)].id.clone();
                self.choose(&id, client);
            }
            Row::Effort => {
                let efforts = models::efforts(&self.listed, &self.model.model);
                let at = efforts.iter().position(|e| *e == self.model.effort).unwrap_or(0);
                let effort = efforts[turn(at, efforts.len(), d)].clone();
                self.model.effort = effort.clone();
                if self.elsewhere {
                    let key = send(client, "", &BTreeMap::from([("model".to_owned(), json!(self.model))]), true);
                    self.sent(key, Sent::NewOnly);
                } else {
                    self.save(BTreeMap::from([("model.effort".to_owned(), json!(effort))]), client);
                }
            }
            Row::Role(r) if !self.elsewhere => {
                let agent = self.agent();
                let (labels, at) = models::role_menu(&self.config, &agent, &self.listed, r);
                if let Some(value) = models::role_value(&agent, &self.listed, r, turn(at, labels.len(), d)) {
                    self.save(BTreeMap::from([(ROLES[r].1.to_owned(), value)]), client);
                }
            }
            Row::ForNew => self.for_new = !self.for_new,
            _ => {}
        }
    }

    /// True closes Settings.
    fn update(&mut self, update: &Update, client: &Handle) -> bool {
        match update {
            Update::SettingsSaved { key, result } if self.keep_request.as_ref() == Some(key) => {
                self.keep_request = None;
                match result {
                    Ok(()) => {
                        self.key.clear();
                        let mut model = self.model.clone();
                        models::set_key(&mut model, &KeyInput::Secret(String::new()));
                        self.model = model.clone();
                        self.key_said = Some(("Kept in Passwords. Checking it with the service…".into(), false));
                        self.save_model(model, client);
                        self.list(client);
                    }
                    Err(e) => {
                        self.key_said = Some((e.clone(), true));
                        self.closing = false;
                    }
                }
                false
            }
            Update::SettingsSaved { key, result } if self.request.as_ref().is_some_and(|(k, _)| k == key) => {
                let (_, sent) = self.request.take().unwrap();
                match (result, sent) {
                    (Ok(()), Sent::Model(_, values)) => {
                        put(&mut self.config, &values);
                        self.elsewhere = false;
                        (self.said, self.bad) = (saved(self.for_new), false);
                    }
                    (Ok(()), Sent::Values(values)) => {
                        put(&mut self.config, &values);
                        (self.said, self.bad) = (saved(self.for_new), false);
                    }
                    (Ok(()), Sent::NewOnly) => (self.said, self.bad) = ("Saved for new sessions.".into(), false),
                    // A conversation stays on its service: the model is new sessions'.
                    (Err(e), Sent::Model(model, values)) if e.contains("start a new session") => {
                        self.elsewhere = true;
                        self.model = model;
                        let key = send(client, "", &values, true);
                        self.sent(key, Sent::NewOnly);
                        return false;
                    }
                    (Err(e), sent) => {
                        if matches!(sent, Sent::Model(..)) {
                            // Shown as it is, not as asked.
                            self.model = self.agent();
                            self.show_model();
                        }
                        (self.said, self.bad) = (models::friendly(e, false), true);
                        self.closing = false;
                        return false;
                    }
                }
                self.closing && self.keep_request.is_none() && self.request.is_none()
            }
            Update::ServiceModels { key, models: result } if self.listing.as_ref() == Some(key) => {
                self.listing = None;
                let keyed = self.model.provider == Provider::Openai;
                self.listed_ok = result.is_ok();
                let checking = self.key_said.as_ref().is_some_and(|(s, _)| s.contains("Checking"));
                match result {
                    Ok(listed) => {
                        if keyed && checking {
                            self.key_said = Some((format!("{} took the key. It’s kept in Passwords.", models::service_name(&self.model)), false));
                        }
                        self.listed = listed.clone();
                    }
                    // No key yet: the line under the key says to paste one.
                    Err(e) if keyed && e.contains("needs your API key: paste") => {
                        if checking {
                            self.key_said = None;
                        }
                    }
                    Err(e) => {
                        let e = models::friendly(e, false);
                        if keyed {
                            self.key_said = Some((e, true));
                        } else {
                            (self.said, self.bad) = (e, true);
                        }
                        if self.listed.is_empty() {
                            self.listed = models::known(&self.model);
                        }
                    }
                }
                false
            }
            _ => false,
        }
    }

    fn paste(&mut self, text: &str) {
        if let Some(field) = self.text() {
            field.push_str(text.trim_matches(['\n', '\r']));
            self.edited = true;
        }
    }

    /// True closes Settings.
    fn key(&mut self, key: KeyEvent, view: &View, client: &Handle) -> bool {
        if self.picker.is_some() {
            let found = self.found();
            let Some((query, selected)) = &mut self.picker else { return false };
            match key.code {
                KeyCode::Esc => self.picker = None,
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => *selected = (*selected + 1).min(found.len().saturating_sub(1)),
                KeyCode::Enter => {
                    let id = found.get(*selected).map(|l| l.id.clone());
                    self.picker = None;
                    if let Some(id) = id {
                        self.choose(&id, client);
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
            }
            return false;
        }
        if self.request.is_some() || self.keep_request.is_some() {
            // Answered in a moment: Esc closes once it is.
            self.closing |= key.code == KeyCode::Esc;
            return false;
        }
        self.follow(view);
        match key.code {
            KeyCode::Esc => {
                if !self.commit(client) {
                    return false;
                }
                if self.request.is_some() || self.keep_request.is_some() {
                    self.closing = true;
                    return false;
                }
                return true;
            }
            KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab => {
                if self.commit(client) {
                    let count = self.rows().len();
                    self.at = turn(self.at.min(count - 1), count, if matches!(key.code, KeyCode::Up | KeyCode::BackTab) { -1 } else { 1 });
                }
            }
            KeyCode::Enter if self.row() == Row::Model && !self.typed_model() => {
                let at = self.choices().iter().position(|l| l.id == self.model.model).unwrap_or(0);
                self.picker = Some((String::new(), at));
            }
            KeyCode::Enter if self.is_text() => {
                self.commit(client);
            }
            _ if self.is_text() => {
                if let Some(field) = self.text()
                    && typed(field, &key, false)
                {
                    self.edited = true;
                }
            }
            _ => {
                if let Some(d) = delta(&key) {
                    self.change(d, client);
                }
            }
        }
        false
    }

    /// The key's line: where it's kept, or what's wrong with it.
    fn key_line(&self) -> (String, bool) {
        if let Some(said) = &self.key_said {
            return said.clone();
        }
        let get = olo_coder_core::providers::of(&self.model).map(|k| format!(" Get one at {}", k.keys_page)).unwrap_or_default();
        match models::key_state(&self.model) {
            KeyState::NotNeeded => (String::new(), false),
            KeyState::Kept(title) => (format!("Kept in Passwords as “{title}”. Paste a new one to replace it."), false),
            KeyState::Unset(title) if self.listed_ok => (format!("Kept in Passwords as “{title}”. Paste a new one to replace it."), false),
            KeyState::Unset(_) if self.listing.is_some() => (String::new(), false),
            KeyState::Unset(_) => (format!("Paste your {} API key here, then Return.{get}", models::service_name(&self.model)), false),
            KeyState::Reference(r) => (format!("Read from {r} in Passwords."), false),
            KeyState::Variable(v) => (format!("Read from ${v} in the Coder service’s environment."), false),
            KeyState::Missing => ("None: fine for a server that doesn’t ask for one.".into(), false),
        }
    }

    fn draw(&self, frame: &mut Frame, area: Rect) {
        if let Some((query, selected)) = &self.picker {
            let found: Vec<String> = self.found().into_iter().map(|l| if l.name == l.id { l.name } else { format!("{}  ({})", l.name, l.id) }).collect();
            frame.render_stateful_widget(
                List::new(found).block(Block::bordered().title(format!(" Model · type to find: {query} · Enter choose · Esc back "))).highlight_symbol("› ").highlight_style(PICKED),
                area,
                &mut ListState::default().with_selected(Some(*selected)),
            );
            return;
        }
        const LABEL: usize = 18;
        let width = area.width.saturating_sub(2) as usize;
        let indent = LABEL + 3;
        let model = &self.model;
        let rows = self.rows();
        let at = self.at.min(rows.len() - 1);
        let mut lines: Vec<(Option<usize>, Line<'static>)> = notes(SECTIONS[0].2, 0, width, DIM);
        lines.push((None, Line::default()));
        let text = |value: &str, hint: &str| if value.is_empty() { vec![Span::styled(hint.to_owned(), DIM)] } else { vec![Span::raw(value.to_owned())] };
        for (i, row) in rows.iter().enumerate() {
            let chosen = i == at;
            match row {
                Row::Service => {
                    lines.push((Some(i), row_line(chosen, "Service:", LABEL, choice(&models::services()[models::service(model)]))));
                    if self.elsewhere {
                        let here = models::service_name(&self.agent());
                        lines.extend(notes(&format!("New sessions start on {}. This conversation stays on {here}: /new starts one.", models::service_name(model)), indent, width, DIM));
                    }
                }
                Row::Address => {
                    lines.push((Some(i), row_line(chosen, "Address:", LABEL, text(&self.address, "https://api.example.com/v1"))));
                    lines.extend(notes("Its OpenAI-compatible API root, which ends in /v1 as a rule.", indent, width, DIM));
                }
                Row::Key => {
                    let shown = match models::key_input(&self.key) {
                        KeyInput::Secret(s) => vec![Span::raw("•".repeat(s.chars().count().min(32)))],
                        _ => text(&self.key, "Paste your API key"),
                    };
                    lines.push((Some(i), row_line(chosen, "API key:", LABEL, shown)));
                    let (line, bad) = self.key_line();
                    lines.extend(notes(&line, indent, width, if bad { BAD } else { DIM }));
                }
                Row::Model => {
                    let value = if self.typed_model() { text(&self.model_id, "Its id, as the service names it") } else { choice(&models::model_name(model, &self.listed, &model.model)) };
                    lines.push((Some(i), row_line(chosen, "Model:", LABEL, value)));
                    if self.listing.is_some() {
                        lines.extend(notes("Asking the service for its models…", indent, width, DIM));
                    } else if chosen && !self.typed_model() {
                        lines.extend(notes("Return lists them all, to find one by name.", indent, width, DIM));
                    }
                }
                Row::Effort => {
                    lines.push((Some(i), row_line(chosen, "Reasoning:", LABEL, choice(&models::effort_name(&model.effort)))));
                    if !self.custom() {
                        let says = format!("Reads up to {} tokens of context and writes up to {} a reply.", models::tokens(model.context_window), models::tokens(model.max_output_tokens as u64));
                        lines.extend(notes(&says, indent, width, DIM));
                    }
                }
                Row::Window | Row::Output => {
                    let (label, value, path) =
                        if *row == Row::Window { ("Context window:", &self.window, "model.context_window") } else { ("Replies, at most:", &self.output, "model.max_output_tokens") };
                    let mut spans = vec![Span::raw(value.clone())];
                    let unit = options::unit(path, value);
                    if !unit.is_empty() {
                        spans.push(Span::styled(format!("  {unit}"), if chosen { PICKED } else { DIM }));
                    }
                    lines.push((Some(i), row_line(chosen, label, LABEL, spans)));
                }
                Row::Role(r) => {
                    if *r == 0 {
                        lines.push((None, Line::default()));
                        lines.push((None, Line::styled("Other jobs", Style::new().add_modifier(Modifier::BOLD))));
                        lines.extend(notes("Which model does each of the agent’s other jobs. The supervisor works only while it’s on (Autonomy).", 0, width, DIM));
                    }
                    let (labels, k) = models::role_menu(&self.config, &self.agent(), &self.listed, *r);
                    lines.push((Some(i), row_line(chosen, &format!("{}:", ROLES[*r].0), LABEL, choice(&labels[k]))));
                }
                Row::ForNew => {
                    lines.push((None, Line::default()));
                    let line = Line::raw(format!("{} [{}] Also for new sessions", if chosen { "›" } else { " " }, if self.for_new { "x" } else { " " }));
                    lines.push((Some(i), if chosen { line.style(PICKED) } else { line }));
                }
            }
        }
        page(frame, area, " Model · ↑↓ move · ←→ change · Esc done ".into(), lines, at, &self.said, self.bad);
    }
}

impl Panel {
    pub fn resume(&mut self) {
        match self {
            Self::Options(p) => {
                p.request = None;
                p.closing = false;
            }
            Self::Model(p) => {
                p.request = None;
                p.keep_request = None;
                p.listing = None;
                p.closing = false;
            }
            Self::Entry { key, busy, .. } => {
                *busy = false;
                *key = olo_coder_core::id();
            }
            Self::Menu { .. } => {}
        }
    }

    pub fn settings(view: &View) -> Self {
        let mut items: Vec<(String, String)> = SECTIONS.iter().map(|(id, title, _)| ((*title).to_owned(), format!("@section:{id}"))).collect();
        items.extend([("Connections…", "@connections"), ("Start a subagent…", "@agent"), ("Message another session…", "@message")].map(|(l, c)| (l.to_owned(), c.to_owned())));
        if let Some(s) = &view.session {
            for (label, key, on) in [("Agent Screens", "olo.screens", s.config.olo.screens), ("Olo Terminal titles", "olo.terminal", s.config.olo.terminal)] {
                items.push((format!("{label}: {}", if on { "On" } else { "Off" }), format!("/set {key} {}", !on)));
            }
        }
        Self::menu("Settings", items)
    }

    fn menu(title: &str, items: Vec<(String, String)>) -> Self {
        Self::Menu { title: title.into(), items, selected: 0, query: String::new() }
    }

    /// True closes the overlay.
    pub fn update(&mut self, update: &Update, client: &Handle) -> bool {
        match self {
            Self::Options(p) => return p.update(update),
            Self::Model(p) => return p.update(update, client),
            _ => {}
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
        false
    }

    pub fn paste(&mut self, text: &str) {
        match self {
            Self::Entry { fields, at, busy, .. } => {
                if !*busy && !fields[*at].select {
                    fields[*at].value.push_str(text);
                }
            }
            Self::Options(p) => p.paste(text),
            Self::Model(p) => p.paste(text),
            Self::Menu { .. } => {}
        }
    }

    /// True closes the overlay. Settings remain event-driven while open.
    pub fn key(&mut self, key: KeyEvent, view: &View, client: &Handle) -> bool {
        match self {
            Self::Options(p) => return p.key(key, view, client),
            Self::Model(p) => return p.key(key, view, client),
            _ => {}
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
                            f.value = f.choices[turn(i, f.choices.len(), if key.code == KeyCode::Left { -1 } else { 1 })].clone();
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
                    return self.open(&command, view, client);
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
            Self::Options(_) | Self::Model(_) => {}
        }
        false
    }

    /// Does what a menu's item says: a page, a form, another menu, or a
    /// command (true: done, Settings closes).
    fn open(&mut self, command: &str, view: &View, client: &Handle) -> bool {
        use crate::form::{Field, Submission};
        let session = view.id.clone();
        if command == "@section:model" {
            if let Some(mut page) = ModelPage::new(view) {
                page.list(client);
                *self = Self::Model(Box::new(page));
            }
        } else if let Some(section) = command.strip_prefix("@section:") {
            if let Some(page) = OptionsPage::new(section, view) {
                *self = Self::Options(Box::new(page));
            }
        } else if command == "@connections" {
            let mut entries = vec![
                ("Add an MCP server…".to_owned(), "@mcp".to_owned()),
                ("Connect Slack…".into(), "@slack".into()),
                ("Share a password field…".into(), "@password".into()),
                ("Connect Logfire (US)".into(), "/integrations setup {\"preset\":\"logfire\"}".into()),
                ("Connect Logfire (EU)".into(), "/integrations setup {\"preset\":\"logfire-eu\"}".into()),
            ];
            for server in view.session.iter().flat_map(|s| &s.config.mcp) {
                entries.push((server.name.clone(), format!("@connection:{}", server.name)));
            }
            *self = Self::menu("Connections", entries);
        } else if let Some(name) = command.strip_prefix("@connection:") {
            let actions = [("Test it", "test"), ("Show its tools", "inspect"), ("Sign in", "login"), ("Sign out", "logout"), ("Remove it", "remove")];
            *self = Self::menu(name, actions.iter().map(|(label, action)| ((*label).into(), format!("/integrations {action} {}", json!({"name":name})))).collect());
        } else if matches!(command, "@agent" | "@message" | "@password" | "@slack" | "@mcp") {
            let (title, description, fields, submission) = match command {
                "@agent" => {
                    let models = models::choices(view, None);
                    let labels = std::iter::once("Same as this session".to_owned()).chain(models.iter().map(|(name, _)| name.clone()));
                    let fields = vec![
                        Field::new("Task:", "").multiline(5),
                        Field::new("Subfolder:", "").optional(),
                        Field::new("Model:", "Same as this session").choices(labels),
                        Field::new("Reasoning:", "Use model settings").choices(std::iter::once("Use model settings".to_owned()).chain(models::effort_names())),
                        Field::new("May:", "Only read").choices(["Only read", "Change files"]),
                    ];
                    ("Start a Subagent", "A helper with a task of its own. This session’s rules for approval still apply to it.", fields, Submission::Agent { session, models })
                }
                "@message" => (
                    "Message Another Session",
                    "Choose an instance and search sessions by name.",
                    vec![
                        Field::new("Instance:", "This service").choices(std::iter::once("This service".to_owned()).chain(view.peers.iter().map(|p| p.name.clone()))),
                        Field::new("Search:", "").optional(),
                    ],
                    Submission::Recipient { dir: Default::default(), session, peer: None, items: vec![] },
                ),
                "@password" => (
                    "Share a Password Field",
                    "Search names, then choose one field. Reading a shared value sends it to the model and the transcript.",
                    vec![Field::new("Search:", "").optional()],
                    Submission::Password { dir: Default::default(), session, phase: 0, items: vec![] },
                ),
                "@slack" => (
                    "Connect Slack",
                    "Use a registered Slack app. Its secret stays in the service environment. Callback: http://127.0.0.1:8787/callback",
                    vec![Field::new("Client ID:", ""), Field::new("Secret variable:", "SLACK_CLIENT_SECRET")],
                    Submission::Integration { session, action: "setup_slack".into(), keys: vec!["client_id".into(), "client_secret_env".into()] },
                ),
                _ => (
                    "Add an MCP Server",
                    "A local program or an HTTPS address. Give references to secrets, never their values.",
                    vec![
                        Field::new("Name:", ""),
                        Field::new("Program or URL:", ""),
                        Field::new("Arguments (JSON):", "[]"),
                        Field::new("Sign-in:", "none").choices(["none", "token", "password", "oauth"]),
                        Field::new("Token reference:", "").optional(),
                        Field::new("OAuth client ID:", "").optional(),
                        Field::new("Secret variable:", "").optional(),
                    ],
                    Submission::Integration {
                        session,
                        action: "configure_fields".into(),
                        keys: ["name", "address", "args", "auth", "reference", "client_id", "client_secret_env"].iter().map(|s| (*s).into()).collect(),
                    },
                ),
            };
            *self =
                Self::Entry { key: olo_coder_core::id(), title: title.into(), description: description.into(), fields, submission: Box::new(submission), at: 0, status: String::new(), busy: false };
        } else {
            client.submit(command.to_owned());
            return true;
        }
        false
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(Clear, area);
        match self {
            Self::Entry { title, description, fields, at, status, .. } => {
                let mut lines = vec![Line::from(description.clone()), Line::default()];
                lines.extend(fields.iter().enumerate().map(|(i, f)| {
                    let value = if f.select { choice(&f.value) } else { vec![Span::raw(f.value.clone())] };
                    row_line(i == *at, &f.label, 20, value)
                }));
                lines.push(Line::default());
                lines.push(Line::styled(status.clone(), DIM));
                frame.render_widget(
                    Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }).block(Block::bordered().title(format!(" {title} · ↑↓ field · ←→ choice · F2 done · Esc cancel "))),
                    area,
                );
            }
            Self::Menu { title, items, selected, query } => {
                // A section says what it's for beside its name.
                // (The sections' names in a column: the other items have
                // nothing beside them.)
                let width = items.iter().filter(|(_, c)| c.starts_with("@section:")).map(|(l, _)| l.chars().count()).max().unwrap_or(0);
                let rows: Vec<Line> = menu_matches(items, query)
                    .into_iter()
                    .map(|(label, command)| {
                        let about = command.strip_prefix("@section:").and_then(|id| SECTIONS.iter().find(|(s, ..)| *s == id)).map_or("", |(_, _, a)| *a);
                        if about.is_empty() { Line::raw(label.clone()) } else { Line::from(vec![Span::raw(format!("{label:width$}  ")), Span::styled(about.to_owned(), DIM)]) }
                    })
                    .collect();
                let filter = if query.is_empty() { "type to find".to_owned() } else { format!("finding “{query}”") };
                frame.render_stateful_widget(
                    List::new(rows).block(Block::bordered().title(format!(" {title} · {filter} · Enter open · Esc close "))).highlight_symbol("› ").highlight_style(PICKED),
                    area,
                    &mut ListState::default().with_selected(Some(*selected)),
                );
            }
            Self::Options(p) => p.draw(frame, area),
            Self::Model(p) => p.draw(frame, area),
        }
    }
}

fn menu_matches<'a>(items: &'a [(String, String)], query: &str) -> Vec<&'a (String, String)> {
    let query = query.to_lowercase();
    items.iter().filter(|v| v.0.to_lowercase().contains(&query)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_coder_core::Session;

    fn view() -> View {
        thread_local! {
            static SESSION: Session = Session::new("/var/tmp".into(), olo_coder_core::Config::default());
        }
        let session = SESSION.with(Clone::clone);
        View { id: session.id.clone(), session: Some(session), ..Default::default() }
    }
    fn client() -> (Handle, std::sync::mpsc::Receiver<Action>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (Handle { tx: tx.into() }, rx)
    }
    fn press(panel: &mut Panel, client: &Handle, code: KeyCode) -> bool {
        panel.key(KeyEvent::new(code, KeyModifiers::NONE), &view(), client)
    }
    fn rendered(panel: &Panel, width: u16, height: u16) -> String {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| panel.draw(f, f.area())).unwrap();
        let buf = terminal.backend().buffer();
        (0..height).map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }
    /// The settings a request sends.
    fn sent(rx: &std::sync::mpsc::Receiver<Action>) -> (String, Value) {
        loop {
            match rx.try_recv().expect("a request") {
                Action::SubmitSettings { key, text } => return (key, serde_json::from_str(text.trim_start_matches("/api ")).unwrap()),
                _ => continue,
            }
        }
    }

    #[test]
    fn a_choice_applies_as_it_changes_and_esc_closes_once_taken() {
        let (client, rx) = client();
        let mut panel = Panel::Options(Box::new(OptionsPage::new("safety", &view()).unwrap()));
        // Actions: Ask me first → the next.
        assert!(!press(&mut panel, &client, KeyCode::Right));
        let (key, request) = sent(&rx);
        assert_eq!(request["op"], "configure_fields");
        assert_eq!(request["defaults"], true);
        assert_eq!(request["values"].as_object().unwrap().len(), 1);
        // Esc waits for the answer, then closes.
        assert!(!press(&mut panel, &client, KeyCode::Esc));
        assert!(panel.update(&Update::SettingsSaved { key, result: Ok(()) }, &client));
    }

    #[test]
    fn typing_applies_on_moving_on_and_a_bad_value_keeps_the_field() {
        let (client, rx) = client();
        let mut panel = Panel::Options(Box::new(OptionsPage::new("budgets", &view()).unwrap()));
        press(&mut panel, &client, KeyCode::Backspace);
        panel.paste("x");
        press(&mut panel, &client, KeyCode::Down);
        let Panel::Options(p) = &panel else { unreachable!() };
        assert_eq!(p.at, 0, "stays on the field it can't save");
        assert!(p.bad);
        assert!(rx.try_recv().is_err());
        // Fixed: it applies and moves on.
        let Panel::Options(p) = &mut panel else { unreachable!() };
        p.values[0] = "77".into();
        p.edited = true;
        press(&mut panel, &client, KeyCode::Down);
        let (_, request) = sent(&rx);
        assert_eq!(request["values"]["max_turns"], 77);
        let Panel::Options(p) = &panel else { unreachable!() };
        assert_eq!(p.at, 1);
    }

    #[test]
    fn a_rejected_change_says_why_and_unrelated_answers_are_ignored() {
        let (client, rx) = client();
        let mut panel = Panel::Options(Box::new(OptionsPage::new("safety", &view()).unwrap()));
        press(&mut panel, &client, KeyCode::Right);
        let (key, _) = sent(&rx);
        assert!(!panel.update(&Update::SettingsSaved { key: "other".into(), result: Ok(()) }, &client));
        assert!(!panel.update(&Update::SettingsSaved { key, result: Err("Error: stop the running turn before this operation".into()) }, &client));
        let Panel::Options(p) = &panel else { unreachable!() };
        assert_eq!(p.said, "The agent is working: its model changes once it stops.");
        assert!(p.request.is_none());
    }

    #[test]
    fn headings_are_passed_over_and_long_values_keep_the_chosen_field_and_status_in_view() {
        let (client, _rx) = client();
        let mut panel = Panel::Options(Box::new(OptionsPage::new("autonomy", &view()).unwrap()));
        for _ in 0..13 {
            press(&mut panel, &client, KeyCode::Down);
        }
        let Panel::Options(p) = &mut panel else { unreachable!() };
        let specs = p.specs();
        assert!(!specs[p.at].path.is_empty() || p.at == specs.len());
        let guidance = specs.iter().position(|s| s.path == "supervisor.guidance").unwrap();
        p.values[guidance] = "Long guidance wrapping across the terminal. ".repeat(80);
        p.at = specs.iter().position(|s| s.path == "supervisor.stall_seconds").unwrap();
        p.said = "Save rejected".into();
        for (width, height) in [(80, 16), (48, 12)] {
            let shown = rendered(&panel, width, height);
            assert!(shown.contains("Stalled after"), "{shown}");
            assert!(shown.contains("Save rejected"), "{shown}");
        }
    }

    #[test]
    fn multiline_values_keep_their_lines_and_show_the_end() {
        let mut page = OptionsPage::new("instructions", &view()).unwrap();
        page.values[0] = (0..30).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        page.said = "Save rejected".into();
        let shown = rendered(&Panel::Options(Box::new(page)), 80, 14);
        assert!(shown.contains("line 29"), "{shown}");
        assert!(!shown.contains("line 0line 1"), "newlines must remain visible: {shown}");
        assert!(shown.contains("Save rejected"), "{shown}");
    }

    #[test]
    fn the_model_page_keeps_a_pasted_key_before_the_model_uses_it() {
        let (client, rx) = client();
        let mut page = ModelPage::new(&view()).unwrap();
        // A custom service: its key is typed.
        let custom = models::services().len() - 1;
        page.model = models::service_model(custom, &page.model);
        page.show_model();
        page.at = page.rows().iter().position(|r| *r == Row::Key).unwrap();
        let mut panel = Panel::Model(Box::new(page));
        panel.paste("sk-test-0123456789abcdef");
        assert!(rendered(&panel, 90, 30).contains("••••"), "the key isn't shown");
        assert!(!serde_json::to_string(&panel).unwrap().contains("sk-test"), "nor kept for an upgrade");
        press(&mut panel, &client, KeyCode::Enter);
        let Action::KeepKey { key, secret, .. } = rx.try_recv().unwrap() else { panic!("keeps the key first") };
        assert_eq!(secret, "sk-test-0123456789abcdef");
        assert!(!panel.update(&Update::SettingsSaved { key, result: Ok(()) }, &client));
        let (_, request) = sent(&rx);
        assert!(request["values"]["model"]["endpoint"].is_object());
        let Panel::Model(p) = &panel else { unreachable!() };
        assert!(p.key.is_empty());
    }

    #[test]
    fn a_model_on_another_service_goes_to_new_sessions() {
        let (client, rx) = client();
        let mut panel = Panel::Model(Box::new(ModelPage::new(&view()).unwrap()));
        press(&mut panel, &client, KeyCode::Right);
        let (key, first) = sent(&rx);
        // The reviewer was the agent's own model: it goes with it.
        assert_eq!(first["values"]["reviewer"]["provider"], first["values"]["model"]["provider"]);
        assert_eq!(first["values"]["reviewer"]["endpoint"], first["values"]["model"]["endpoint"]);
        assert!(!panel.update(&Update::SettingsSaved { key, result: Err("Error: this conversation runs on the token pool: start a new session to use another service".into()) }, &client));
        let (_, request) = sent(&rx);
        assert_eq!(request["session"], "");
        assert!(request["values"]["reviewer"].is_object(), "and goes to new sessions with it");
        let Panel::Model(p) = &panel else { unreachable!() };
        assert!(p.elsewhere);
        assert!(rendered(&panel, 100, 30).contains("New sessions start on"));
    }

    #[test]
    fn menus_say_what_each_section_is_for() {
        let shown = rendered(&Panel::settings(&view()), 110, 20);
        assert!(shown.contains("Model"), "{shown}");
        assert!(shown.contains("What the agent runs on"), "{shown}");
        assert!(shown.contains("Connections…"), "{shown}");
    }

    #[test]
    fn words_wrap_between_words() {
        assert_eq!(wrap("one two three", 8), ["one two", "three"]);
        assert_eq!(wrap("abcdefghijkl", 8), ["abcdefgh", "ijkl"]);
        assert_eq!(wrap("a\nb", 8), ["a", "b"]);
    }
}
