//! Named models are configured once and selected by role throughout Coder.
use crate::client::{Action, Handle};
use olo_coder_core::{Command, ModelConfig, Provider, profiles::ModelProfile, rpc};
use olo_ui::{
    WindowOptions,
    gpui::{self, AppContext as _, BorrowAppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Window, div, px},
    metrics as m,
    widgets::{self, Button, MenuField, StringView, TextControl, TextField},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
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
#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot {
    key: String,
    session: String,
    profile: ModelProfile,
    ids: Vec<String>,
    selected: Option<String>,
    role: usize,
    #[serde(default)]
    numeric: Option<(String, String)>,
    #[serde(default)]
    filter: String,
}
#[derive(Default)]
struct Windows(Vec<gpui::WeakEntity<Settings>>);
impl gpui::Global for Windows {}
pub fn snapshots(cx: &gpui::App) -> Vec<Snapshot> {
    cx.try_global::<Windows>().into_iter().flat_map(|s| &s.0).filter_map(|w| w.upgrade()).map(|w| w.read(cx).snapshot(cx)).collect()
}
pub fn changed(cx: &mut gpui::App, profiles: &[ModelProfile]) {
    let windows = cx.try_global::<Windows>().into_iter().flat_map(|s| &s.0).filter_map(|w| w.upgrade()).collect::<Vec<_>>();
    for window in windows {
        window.update(cx, |s, cx| {
            s.profiles = profiles.to_vec();
            if s.selected.as_ref().is_some_and(|id| !profiles.iter().any(|p| &p.id == id)) {
                s.selected = None;
            }
            cx.notify();
        });
    }
}
pub fn restore(cx: &mut gpui::App, dir: PathBuf, client: Handle, states: Vec<Snapshot>) {
    for state in states {
        open_state(cx, dir.clone(), client.clone(), state);
    }
}
pub fn open(cx: &mut gpui::App, dir: PathBuf, session: String, client: Handle, model: ModelConfig) {
    open_state(
        cx,
        dir,
        client,
        Snapshot {
            key: olo_coder_core::id(),
            session,
            profile: ModelProfile { id: olo_coder_core::id(), name: "New model".into(), model },
            ids: vec![],
            selected: None,
            role: 0,
            numeric: None,
            filter: String::new(),
        },
    );
}
struct Settings {
    key: String,
    dir: PathBuf,
    session: String,
    client: Handle,
    profiles: Vec<ModelProfile>,
    selected: Option<String>,
    profile: ModelProfile,
    name: Entity<TextField>,
    url: Entity<TextField>,
    token: Entity<TextField>,
    context: Entity<TextField>,
    output: Entity<TextField>,
    filter: Entity<TextField>,
    filtered: Vec<String>,
    ids: Vec<String>,
    source: String,
    role: usize,
    busy: bool,
    fetching: bool,
    generation: u64,
    status: String,
    _subscriptions: Vec<gpui::Subscription>,
}
impl Settings {
    fn filter_models(&mut self, cx: &gpui::App) {
        let query = self.filter.read(cx).text().to_lowercase();
        self.filtered = self.ids.iter().filter(|id| id.to_lowercase().contains(&query)).cloned().collect();
    }
    fn source(&self, cx: &gpui::App) -> String {
        format!("{:?}\n{}\n{}", self.profile.model.provider, self.url.read(cx).text(), self.token.read(cx).text())
    }
    fn draft(&self, cx: &gpui::App) -> anyhow::Result<ModelProfile> {
        let mut p = self.profile.clone();
        p.name = self.name.read(cx).text().trim().into();
        p.model.context_window = self.context.read(cx).text().parse()?;
        p.model.max_output_tokens = self.output.read(cx).text().parse()?;
        if p.model.provider == Provider::Openai {
            let e = p.model.endpoint.get_or_insert_with(Default::default);
            e.base_url = self.url.read(cx).text().trim().into();
            e.api_key_env = (!self.token.read(cx).text().trim().is_empty()).then(|| self.token.read(cx).text().trim().into());
        }
        p.validate()?;
        Ok(p)
    }
    fn snapshot(&self, cx: &gpui::App) -> Snapshot {
        let mut profile = self.profile.clone();
        profile.name = self.name.read(cx).text().into();
        if let Some(endpoint) = &mut profile.model.endpoint {
            endpoint.base_url = self.url.read(cx).text().into();
            endpoint.api_key_env = Some(self.token.read(cx).text().into());
        }
        if let Ok(v) = self.context.read(cx).text().parse() {
            profile.model.context_window = v;
        }
        if let Ok(v) = self.output.read(cx).text().parse() {
            profile.model.max_output_tokens = v;
        }
        Snapshot {
            key: self.key.clone(),
            session: self.session.clone(),
            profile,
            ids: if self.source == self.source(cx) { self.ids.clone() } else { vec![] },
            selected: self.selected.clone(),
            role: self.role,
            numeric: Some((self.context.read(cx).text().into(), self.output.read(cx).text().into())),
            filter: self.filter.read(cx).text().into(),
        }
    }
    fn load(&mut self, p: ModelProfile, cx: &mut Context<Self>) {
        self.name.update(cx, |f, cx| f.set_text(p.name.clone(), cx));
        self.url.update(cx, |f, cx| f.set_text(p.model.endpoint.as_ref().map(|e| e.base_url.clone()).unwrap_or_default(), cx));
        self.token.update(cx, |f, cx| f.set_text(p.model.endpoint.as_ref().and_then(|e| e.api_key_env.clone()).unwrap_or_default(), cx));
        self.context.update(cx, |f, cx| f.set_text(p.model.context_window.to_string(), cx));
        self.output.update(cx, |f, cx| f.set_text(p.model.max_output_tokens.to_string(), cx));
        self.ids = vec![p.model.model.clone()];
        self.filter.update(cx, |f, cx| f.set_text("", cx));
        self.filter_models(cx);
        self.profile = p;
        self.source = self.source(cx);
        self.status.clear();
        cx.notify();
    }
    fn request(&mut self, action: &str, cx: &mut Context<Self>) {
        let profile = if matches!(action, "save" | "discover") {
            match self.draft(cx) {
                Ok(p) => Some(p),
                Err(e) => {
                    self.status = e.to_string();
                    cx.notify();
                    return;
                }
            }
        } else {
            None
        };
        if action == "save" && self.source != self.source(cx) {
            self.status = "The endpoint changed. Fetch its models before saving.".into();
            cx.notify();
            return;
        }
        let source = self.source(cx);
        let command = Command::ModelProfiles {
            action: action.into(),
            profile,
            id: self.selected.clone(),
            session: Some(self.session.clone()),
            role: ["primary", "supervisor", "reviewer", "compaction"][self.role].into(),
        };
        self.generation += 1;
        let generation = self.generation;
        let action = action.to_owned();
        let dir = self.dir.clone();
        self.busy = true;
        self.fetching = action == "discover";
        for f in [&self.name, &self.url, &self.token, &self.context, &self.output] {
            f.update(cx, |f, cx| f.set_enabled(false, cx));
        }
        self.status = if action == "discover" { "Fetching the provider’s model catalog…" } else { "Updating model profiles…" }.into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { rpc::call(&dir, &command) }).await;
            let _ = this.update(cx, |s, cx| {
                if s.generation != generation {
                    return;
                }
                s.busy = false;
                for f in [&s.name, &s.url, &s.token, &s.context, &s.output] {
                    f.update(cx, |f, cx| f.set_enabled(true, cx));
                }
                match result {
                    Err(e) => s.status = e.to_string(),
                    Ok(value) => {
                        if let Some(profiles) = value.get("profiles") {
                            s.profiles = serde_json::from_value(profiles.clone()).unwrap_or_default();
                            if action == "save" {
                                s.selected = Some(s.profile.id.clone());
                            }
                            if action == "remove" {
                                s.selected = None;
                                s.profile.id = olo_coder_core::id();
                            }
                            let _ = s.client.tx.send(Action::Profiles);
                        }
                        if let Some(ids) = value["models"].as_array() {
                            s.source = source.clone();
                            s.ids = ids.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect();
                            s.filter_models(cx);
                            if !s.ids.contains(&s.profile.model.model) {
                                s.profile.model.model = s.ids.first().cloned().unwrap_or_default();
                            }
                            s.status = format!("{} models available. Choose a model, then save this profile.", s.ids.len());
                        } else {
                            s.status = match action.as_str() {
                                "save" => "Profile saved. Existing sessions keep their selected configuration.",
                                "apply" => "Selected profile applied to this session.",
                                "remove" => "Profile removed. Existing session configurations are preserved.",
                                _ => "Create a profile or choose one to edit.",
                            }
                            .into();
                        }
                        if action == "apply" {
                            let _ = s.client.tx.send(Action::Refresh);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let new = cx.entity();
        let provider = cx.entity();
        let discover = cx.entity();
        let model = cx.entity();
        let effort_view = cx.entity();
        let save = cx.entity();
        let remove = cx.entity();
        let apply = cx.entity();
        let role = cx.entity();
        let cancel = cx.entity();
        let profiles = self.profiles.clone();
        let mut labels = vec!["New profile".into()];
        labels.extend(profiles.iter().map(|p| p.name.clone().into()));
        let selected = profiles.iter().position(|p| Some(&p.id) == self.selected.as_ref()).map_or(0, |i| i + 1);
        let editable = !self.busy;
        let mut ids = if self.source == self.source(cx) { self.filtered.clone() } else { vec![] };
        // A filter narrows choices, but must not pretend the first match is selected.
        if self.source == self.source(cx) && self.ids.contains(&self.profile.model.model) && !ids.contains(&self.profile.model.model) {
            ids.insert(0, self.profile.model.model.clone());
        }
        let at = ids.iter().position(|id| id == &self.profile.model.model).unwrap_or(0);
        let choices = if ids.is_empty() { vec![if self.ids.is_empty() { "Fetch models to choose…" } else { "No matching models" }.into()] } else { ids.iter().cloned().map(Into::into).collect() };
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .p(px(m::WINDOW_INSET))
            .child(StringView::new("Define models once. Choose a saved profile for the primary agent, supervisor, approval reviewer or compaction.").wrap(true).dim(true))
            .child(
                div()
                    .flex()
                    .gap(px(m::SPACING))
                    .child(MenuField::new("saved-profiles", labels, selected).label("Profile:").divider(120.0).width(380.0).enabled(editable).on_change(move |i, _, cx| {
                        me.update(cx, |s, cx| {
                            if i == 0 {
                                s.selected = None;
                                let mut p = s.profile.clone();
                                p.id = olo_coder_core::id();
                                p.name = "New model".into();
                                s.load(p, cx);
                            } else {
                                s.selected = Some(profiles[i - 1].id.clone());
                                s.load(profiles[i - 1].clone(), cx);
                            }
                        })
                    }))
                    .child(Button::new("New").enabled(editable).on_click(move |_, cx| {
                        new.update(cx, |s, cx| {
                            s.selected = None;
                            let p = ModelProfile { id: olo_coder_core::id(), name: "New model".into(), model: ModelConfig::default() };
                            s.load(p, cx);
                        })
                    })),
            )
            .child(TextControl::new(self.name.clone()).label("Name:").divider(120.0))
            .child(
                MenuField::new("profile-provider", vec!["Codex · Token Pool".into(), "OpenAI-compatible endpoint".into()], usize::from(self.profile.model.provider == Provider::Openai))
                    .label("Provider:")
                    .divider(120.0)
                    .enabled(editable)
                    .on_change(move |i, _, cx| {
                        provider.update(cx, |s, cx| {
                            let p = if i == 0 { Provider::Codex } else { Provider::Openai };
                            if s.profile.model.provider != p {
                                let mut profile = s.profile.clone();
                                profile.name = s.name.read(cx).text().into();
                                let context = s.context.read(cx).text().to_owned();
                                let output = s.output.read(cx).text().to_owned();
                                profile.model = ModelConfig::for_provider(p);
                                s.load(profile, cx);
                                s.context.update(cx, |f, cx| f.set_text(context, cx));
                                s.output.update(cx, |f, cx| f.set_text(output, cx));
                                s.ids.clear();
                            }
                        })
                    }),
            );
        if self.profile.model.provider == Provider::Openai {
            body = body.child(TextControl::new(self.url.clone()).label("API root:").divider(120.0)).child(TextControl::new(self.token.clone()).label("Key variable:").divider(120.0));
        }
        body = body
            .child(div().flex().justify_end().child(Button::new("Fetch Models").enabled(editable).on_click(move |_, cx| discover.update(cx, |s, cx| s.request("discover", cx)))))
            .child(StringView::new(format!("Selected model: {}", self.profile.model.model)).wrap(true).dim(true))
            .child(TextControl::new(self.filter.clone()).label("Filter models:").divider(120.0))
            .child(MenuField::new("profile-model", choices, at).label("Model:").divider(120.0).width(440.0).enabled(editable && !ids.is_empty()).on_change(move |i, _, cx| {
                model.update(cx, |s, cx| {
                    s.profile.model.model = ids[i].clone();
                    cx.notify();
                })
            }))
            .child(MenuField::new("profile-effort", EFFORTS.iter().map(|s| (*s).into()).collect(), effort(&self.profile.model.effort)).label("Reasoning:").divider(120.0).enabled(editable).on_change(
                move |i, _, cx| {
                    effort_view.update(cx, |s, cx| {
                        s.profile.model.effort = effort_value(i);
                        cx.notify();
                    })
                },
            ))
            .child(TextControl::new(self.context.clone()).label("Context tokens:").divider(120.0))
            .child(TextControl::new(self.output.clone()).label("Output tokens:").divider(120.0))
            .child(StringView::new(self.status.clone()).wrap(true).dim(true))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(m::SPACING))
                    .child(Button::new("Cancel Fetch").enabled(self.busy && self.fetching).on_click(move |_, cx| {
                        cancel.update(cx, |s, cx| {
                            s.generation += 1;
                            s.busy = false;
                            for f in [&s.name, &s.url, &s.token, &s.context, &s.output] {
                                f.update(cx, |f, cx| f.set_enabled(true, cx));
                            }
                            s.status = "Request dismissed; no profile changes were applied locally.".into();
                            cx.notify();
                        })
                    }))
                    .child(Button::new("Delete Profile").enabled(editable && self.selected.is_some()).on_click(move |_, cx| remove.update(cx, |s, cx| s.request("remove", cx))))
                    .child(
                        Button::new("Save Profile").enabled(editable && !self.ids.is_empty() && self.source == self.source(cx)).on_click(move |_, cx| save.update(cx, |s, cx| s.request("save", cx))),
                    ),
            )
            .child(widgets::separator())
            .child(
                div()
                    .flex()
                    .gap(px(m::SPACING))
                    .child(
                        MenuField::new("profile-role", vec!["Primary agent".into(), "Supervisor".into(), "Approval reviewer".into(), "Compaction".into()], self.role)
                            .label("Use for:")
                            .divider(120.0)
                            .enabled(editable)
                            .on_change(move |i, _, cx| {
                                role.update(cx, |s, cx| {
                                    s.role = i;
                                    cx.notify();
                                })
                            }),
                    )
                    .child(Button::new("Use Saved Profile").enabled(editable && self.selected.is_some()).on_click(move |_, cx| apply.update(cx, |s, cx| s.request("apply", cx)))),
            );
        body
    }
}
fn open_state(cx: &mut gpui::App, dir: PathBuf, client: Handle, state: Snapshot) {
    if cx.try_global::<Windows>().is_none() {
        cx.set_global(Windows::default());
    }
    cx.defer(move |cx| {
        olo_ui::open_window(cx, WindowOptions::new("Model Profiles", 680.0, 200.0).dialog().child().restore_key(state.key.clone()), move |_, cx| {
            let view = cx.new(|cx| {
                let field = |name: &str, text: String, cx: &mut Context<Settings>| {
                    let f = widgets::text_field(cx, |f| f.name(name.to_owned()));
                    f.update(cx, |f, cx| f.set_text(text, cx));
                    f
                };
                let name = field("Profile name", state.profile.name.clone(), cx);
                let url = field("API root", state.profile.model.endpoint.as_ref().map(|e| e.base_url.clone()).unwrap_or_default(), cx);
                let token = field("Key variable", state.profile.model.endpoint.as_ref().and_then(|e| e.api_key_env.clone()).unwrap_or_default(), cx);
                let context = field("Context tokens", state.numeric.as_ref().map(|v| v.0.clone()).unwrap_or_else(|| state.profile.model.context_window.to_string()), cx);
                let output = field("Output tokens", state.numeric.as_ref().map(|v| v.1.clone()).unwrap_or_else(|| state.profile.model.max_output_tokens.to_string()), cx);
                let filter = field("Filter models", state.filter, cx);
                let mut subscriptions: Vec<_> = [&name, &url, &token, &context, &output].iter().map(|f| cx.observe(*f, |_, _, cx| cx.notify())).collect();
                subscriptions.push(cx.observe(&filter, |s: &mut Settings, _, cx| {
                    s.filter_models(cx);
                    cx.notify();
                }));
                let mut s = Settings {
                    key: state.key,
                    dir,
                    session: state.session,
                    client,
                    profiles: vec![],
                    selected: state.selected,
                    profile: state.profile,
                    name,
                    url,
                    token,
                    context,
                    output,
                    filter,
                    filtered: vec![],
                    ids: state.ids,
                    source: String::new(),
                    role: state.role,
                    busy: false,
                    fetching: false,
                    generation: 0,
                    status: String::new(),
                    _subscriptions: subscriptions,
                };
                s.filter_models(cx);
                s.source = s.source(cx);
                s.request("list", cx);
                s
            });
            cx.update_global::<Windows, _>(|w, _| {
                w.0.retain(|v| v.upgrade().is_some());
                w.0.push(view.downgrade());
            });
            view
        });
    });
}
