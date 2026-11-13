//! A form in a dialog of its own (Rename, Commit, Set Goal, Add MCP
//! Server…): its fields, what it submits, Cancel and the action. What it
//! submits is an ordinary API command (`form::Submission`), the same the
//! terminal sends. A form that looks something up first (a password's
//! item, then its field; a session to write to) shows what it found as its
//! next step. The dialog stays until the service says it's done, and says
//! why when it isn't.
use crate::{
    client::{Action, Handle},
    form::{Field, Submission},
};
use olo_kit::{
    Child, Cx, Element, InteractiveElement as _, IntoElement as _, KeyDownEvent, ParentElement as _, Styled as _, View, WindowCx, WindowOptions, canvas, div, metrics as m, px, style, theme,
    widgets::{Button, MenuField, StringView, TextControl, TextField, TextFieldEvent, text_field},
};

#[derive(Clone)]
pub enum Msg {
    Submit,
    Cancel,
    Choose(usize, usize),
    Edited,
    Done(u64, Result<(), String>),
    Found(u64, Result<serde_json::Value, String>),
}

struct Input {
    field: Field,
    text: Child<TextField>,
    chosen: usize,
}

pub struct Dialog {
    /// The window it's over (its `window_key`), and its own title.
    owner: String,
    title: String,
    description: String,
    action: String,
    inputs: Vec<Input>,
    submission: Submission,
    client: Handle,
    said: Option<(String, bool)>,
    busy: bool,
    asked: u64,
}

/// Opens the form over the window `cx` is in (known as `owner`).
pub fn open(cx: &mut WindowCx, owner: &str, title: &str, description: &str, action: &str, fields: Vec<Field>, client: Handle, submission: Submission) {
    let (owner, title_, description, action) = (owner.to_owned(), title.to_owned(), description.to_owned(), action.to_owned());
    let options = WindowOptions { fixed: true, ..WindowOptions::new(title.to_owned(), 500.0, 120.0).dialog() };
    olo_kit::open_window_here(cx, options, move |cx| {
        let inputs = inputs(cx, fields);
        if let Some(first) = inputs.iter().find(|i| i.field.choices.is_empty() && !i.field.select) {
            first.text.focus(cx);
        }
        Dialog { owner, title: title_, description, action, inputs, submission, client, said: None, busy: false, asked: 0 }
    });
}

/// Opens again, over the window `cx` is in, a form an upgrade closed (what
/// `save_state` kept: its fields as they were filled in).
pub fn reopen(cx: &mut WindowCx, state: &serde_json::Value, client: Handle) {
    let (Ok(fields), Ok(submission)) = (serde_json::from_value::<Vec<Field>>(state["fields"].clone()), serde_json::from_value::<Submission>(state["submission"].clone())) else { return };
    let s = |k: &str| state[k].as_str().unwrap_or("").to_owned();
    open(cx, &s("owner"), &s("title"), &s("description"), &s("action"), fields, client, submission);
}

fn inputs(cx: &mut Cx<Dialog>, fields: Vec<Field>) -> Vec<Input> {
    fields
        .into_iter()
        .map(|field| {
            let name = field.label.trim_end_matches(':').to_owned();
            let value = field.value.clone();
            let rows = field.rows;
            let text = text_field(cx, |f| {
                let f = f.name(name).with_text(value);
                if rows > 1 { f.multiline(false) } else { f }
            });
            cx.subscribe(&text, |_: &mut Dialog, e: &TextFieldEvent, cx| {
                if matches!(e, TextFieldEvent::Changed) {
                    cx.send(Msg::Edited);
                }
            });
            if rows > 1 {
                // Read from its beginning.
                text.update(cx, |f, cx| f.set_selection(0..0, cx));
            }
            let chosen = field.choices.iter().position(|c| *c == field.value).unwrap_or(0);
            Input { field, text, chosen }
        })
        .collect()
}

impl Dialog {
    fn values(&self, cx: &Cx<Self>) -> Vec<String> {
        self.inputs.iter().map(|i| if i.field.select { i.field.choices.get(i.chosen).cloned().unwrap_or_default() } else { i.text.read(cx).text().to_owned() }).collect()
    }

    fn ready(&self, cx: &Cx<Self>) -> bool {
        !self.busy && self.inputs.iter().zip(self.values(cx)).all(|(i, v)| !i.field.required || !v.trim().is_empty())
    }

    fn submit(&mut self, cx: &mut Cx<Self>) {
        if !self.ready(cx) {
            return;
        }
        let values = self.values(cx);
        self.submission.prepare(&values);
        let command = self.submission.command(values.clone());
        let api = command.trim_start_matches("/api ").to_owned();
        self.asked += 1;
        let asked = self.asked;
        // A lookup: what it finds is the next step.
        if self.submission.browsing() {
            let (Submission::Password { dir, .. } | Submission::Recipient { dir, .. }) = &self.submission else { return };
            let dir = dir.clone();
            self.busy = true;
            self.said = Some(("Looking…".into(), false));
            cx.work(
                move || {
                    let command: olo_coder_core::Command = serde_json::from_str(&api).map_err(|e| e.to_string())?;
                    olo_coder_core::rpc::call(&dir, &command).map_err(|e| format!("{e:#}"))
                },
                move |r| Msg::Found(asked, r),
            );
            cx.invalidate();
            return;
        }
        if let Err(e) = self.check(&api, &values) {
            self.said = Some((e, true));
            cx.invalidate();
            return;
        }
        let (reply, answer) = tokio::sync::oneshot::channel();
        if self.client.tx.send(Action::SubmitForm { text: command, reply }).is_err() {
            self.said = Some(("The session service isn’t connected. What you wrote is still here.".into(), true));
            cx.invalidate();
            return;
        }
        self.busy = true;
        self.said = Some(("Working…".into(), false));
        cx.work(move || answer.blocking_recv().unwrap_or_else(|_| Err("The connection closed before an answer came. Check the session before trying again.".into())), move |r| Msg::Done(asked, r));
        cx.invalidate();
    }

    /// What the service would refuse, said before asking it.
    fn check(&self, api: &str, values: &[String]) -> Result<(), String> {
        let typed: olo_coder_core::Command = serde_json::from_str(api).map_err(|e| e.to_string())?;
        match typed {
            olo_coder_core::Command::Supervisor { config: Some(config), .. } => config.validate().map_err(|e| e.to_string())?,
            olo_coder_core::Command::Configure { config, .. } => config.validate().map_err(|e| e.to_string())?,
            _ => {}
        }
        if let Submission::Integration { action, .. } = &self.submission {
            if action == "configure_fields" {
                serde_json::from_str::<Vec<String>>(&values[2]).map_err(|_| "Arguments are a JSON list of strings, like [\"--stdio\"].".to_owned())?;
            }
        }
        Ok(())
    }
}

impl View for Dialog {
    type Message = Msg;

    /// Its fields as filled in, to come back as they were after an upgrade.
    fn save_state(&mut self, cx: &mut Cx<Self>) -> Option<String> {
        let values = self.values(cx);
        let fields: Vec<Field> = self.inputs.iter().zip(values).map(|(i, value)| Field { value, ..i.field.clone() }).collect();
        Some(serde_json::json!({"form":"dialog","owner":self.owner,"title":self.title,"description":self.description,"action":self.action,"fields":fields,"submission":self.submission}).to_string())
    }

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Submit => return self.submit(cx),
            Msg::Cancel => return cx.close_window(),
            Msg::Choose(i, at) => {
                if let Some(input) = self.inputs.get_mut(i) {
                    input.chosen = at;
                }
                self.said = None;
            }
            Msg::Edited => {
                if self.said.as_ref().is_some_and(|(_, bad)| *bad) {
                    self.said = None;
                }
            }
            Msg::Done(asked, result) => {
                if asked != self.asked {
                    return;
                }
                self.busy = false;
                match result {
                    Ok(()) => return cx.close_window(),
                    Err(e) => self.said = Some((e, true)),
                }
            }
            Msg::Found(asked, result) => {
                if asked != self.asked {
                    return;
                }
                self.busy = false;
                match result.and_then(|v| crate::form::next_choices(&mut self.submission, v).map_err(|e| e.to_string())) {
                    Ok((fields, action, description)) => {
                        self.inputs = inputs(cx, fields);
                        self.action = action;
                        self.description = description;
                        self.said = None;
                        if let Some(first) = self.inputs.iter().find(|i| !i.field.select) {
                            first.text.focus(cx);
                        }
                    }
                    Err(e) => self.said = Some((e, true)),
                }
            }
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let divider = 130.0;
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .p(px(m::WINDOW_INSET))
            .capture_key_down(cx.listener(|_: &mut Dialog, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" {
                    cx.send(Msg::Cancel);
                    cx.stop_propagation();
                } else if e.keystroke.key == "enter" && (e.keystroke.modifiers.platform || e.keystroke.modifiers.control) {
                    cx.send(Msg::Submit);
                    cx.stop_propagation();
                }
            }))
            .child(StringView::new(self.description.clone()).wrap(true));
        let lh = f32::from(TextField::line_height_for(cx.scale_factor(), style::PLAIN_SIZE));
        for (i, input) in self.inputs.iter().enumerate() {
            let f = &input.field;
            if f.select {
                let options: Vec<olo_kit::SharedString> = if f.choices.is_empty() { vec!["None to choose from".into()] } else { f.choices.iter().map(|c| c.clone().into()).collect() };
                col = col.child(
                    MenuField::new(format!("choice-{i}"), options, input.chosen)
                        .label(f.label.clone())
                        .divider(divider)
                        .enabled(!self.busy && !f.choices.is_empty())
                        .on_change(cx.msg_with(move |at: &usize| Msg::Choose(i, *at))),
                );
            } else if f.rows > 1 {
                let focused = input.text.read(cx).is_focused();
                let well = canvas(move |g, b| {
                    let p = theme::palette();
                    style::fill_rect(g, style::inset(g, b, 2.0), style::c(p.document_background));
                    style::sunken(g, b, &p, focused);
                })
                .paint_key(style::key(focused))
                .absolute()
                .inset_0();
                let h = f.rows as f32 * lh + 2.0 * (2.0 + m::TIGHT);
                col = col.child(
                    div()
                        .flex()
                        .gap(px(m::LABEL_GAP))
                        .child(div().w(px(divider - m::LABEL_GAP)).flex_shrink_0().child(StringView::new(f.label.clone())))
                        .child(div().flex_1().min_w_0().relative().h(px(h)).child(well).child(div().absolute().inset(px(2.0 + m::TIGHT)).child(&input.text))),
                );
            } else {
                col = col.child(TextControl::new(cx, &input.text).label(f.label.clone()).divider(divider));
            }
        }
        if let Some((said, bad)) = &self.said {
            let p = theme::palette();
            let view = StringView::new(said.clone()).wrap(true);
            col = col.child(if *bad { view.color(style::c(p.failure)) } else { view.dim(true) });
        }
        let ready = self.ready(cx);
        col.child(
            div()
                .flex()
                .justify_end()
                .gap(px(m::SPACING))
                .child(Button::new("Cancel").on_click(cx.msg(Msg::Cancel)))
                .child(Button::new(self.action.clone()).default(true).enabled(ready).on_click(cx.msg(Msg::Submit))),
        )
        .into_any_element()
    }
}
