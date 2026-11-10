//! Small native forms. The result is a normal API command, so review, effects
//! and audit history are identical to the terminal and automation interfaces.
use crate::client::Handle;
use olo_ui::{
    WindowOptions,
    gpui::{self, AppContext as _, BorrowAppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _, Window, div, px},
    metrics as m,
    widgets::{self, Button, StringView, TextControl, TextField},
};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Field {
    pub label: String,
    pub value: String,
    pub rows: usize,
    pub required: bool,
}
impl Field {
    pub fn new(label: &'static str, value: impl Into<String>) -> Self {
        Self { label: label.into(), value: value.into(), rows: 1, required: true }
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
    Coordination { operation: String, session: String, message: Option<String> },
    Repository { operation: String, session: String, head: String },
    Guidance { session: String, config: olo_coder_core::Config },
}
impl Submission {
    fn command(&self, v: Vec<String>) -> String {
        let command = match self {
            Self::Coordination { operation, session, message } => match operation.as_str() {
                "rename" => serde_json::json!({"op":"organize","session":session,"title":v[0]}),
                "connect" => serde_json::json!({"op":"link_peer","name":v[0],"directory":v[1],"reciprocal":true}),
                "edit" => serde_json::json!({"op":"edit_message","session":session,"message":message,"text":v[0]}),
                _ => {
                    let (peer, target) = v[0].split_once('/').map(|(p, t)| (Some(p), t)).unwrap_or((None, v[0].as_str()));
                    serde_json::json!({"op":"send_message","session":session,"target":target,"peer":peer,"text":v[1],"delivery":"after_tool","wake":false})
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
            Self::Guidance { session, config } => {
                let mut config = config.clone();
                config.approval_guidance = v[0].clone();
                serde_json::json!({"op":"configure","session":session,"config":config})
            }
        };
        format!("/api {command}")
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
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
    fields: Vec<(Field, Entity<TextField>)>,
    client: Handle,
    title: String,
    action: String,
    description: String,
    submission: Submission,
    _subscriptions: Vec<gpui::Subscription>,
}
impl Form {
    fn ready(&self, cx: &Context<Self>) -> bool {
        self.fields.iter().all(|(spec, field)| !spec.required || !field.read(cx).text().trim().is_empty())
    }
    fn submit(&self, window: &mut Window, cx: &Context<Self>) {
        if self.ready(cx) {
            self.client.submit(self.submission.command(self.fields.iter().map(|(_, f)| f.read(cx).text().to_owned()).collect()));
            window.remove_window();
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
                } else if e.keystroke.key == "enter" && (e.keystroke.modifiers.platform || s.fields.iter().any(|(spec, f)| spec.rows == 1 && f.read(cx).is_focused(w))) {
                    s.submit(w, cx);
                    cx.stop_propagation();
                }
            }))
            .child(StringView::new(self.description.clone()).dim(true));
        for (spec, field) in &self.fields {
            content = content.child(TextControl::new(field.clone()).label(spec.label.clone()).divider(100.0).rows(spec.rows));
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
    open_snapshot(cx, Snapshot { title: title.into(), description: description.into(), action: action.into(), fields, submission }, client);
}
fn open_snapshot(cx: &mut gpui::App, snapshot: Snapshot, client: Handle) {
    if cx.try_global::<Forms>().is_none() {
        cx.set_global(Forms::default());
    }
    cx.defer(move |cx| {
        let Snapshot { title, description, action, fields, submission } = snapshot;
        olo_ui::open_window(cx, WindowOptions::new(title.clone(), 580.0, 200.0).dialog().child(), move |window, cx| {
            let view = cx.new(|cx| {
                let fields: Vec<_> = fields
                    .into_iter()
                    .map(|spec| {
                        let field = widgets::text_field(cx, |f| f.name(spec.label.trim_end_matches(':').to_owned()).multiline(spec.rows > 1));
                        field.update(cx, |f, cx| f.set_text(&spec.value, cx));
                        (spec, field)
                    })
                    .collect();
                let subscriptions = fields.iter().map(|(_, f)| cx.observe(f, |_, _, cx| cx.notify())).collect();
                Form { fields, client, title, action, description, submission, _subscriptions: subscriptions }
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
