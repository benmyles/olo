use crate::client::{self, Action, Handle, Update, View};
use crate::form::{self, Field};
use olo_coder_core::{Delivery, SessionFilter};
use olo_ui::{
    Menu, MenuBar, WindowOptions,
    gpui::{self, AppContext as _, BorrowAppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, canvas, div, px},
    metrics as m,
    widgets::{self, Button, CheckBox, Column, ColumnListView, ListModel, MenuField, ScrollView, StatusBar, StringView, TextControl, TextField, TextFieldEvent, TextView},
};
use std::{collections::HashMap, path::PathBuf};

struct Coder {
    restore_key: String,
    win: Option<gpui::WindowHandle<olo_ui::Frame>>,
    dir: PathBuf,
    workspace: PathBuf,
    drafts: HashMap<String, String>,
    delivery: Delivery,
    page: usize,
    sessions: Entity<ListModel>,
    inbox: Entity<ListModel>,
    search: Entity<TextField>,
    filter: SessionFilter,
    settings_scroll: gpui::ScrollHandle,
    _selection_subscriptions: Vec<gpui::Subscription>,
    _search_subscription: gpui::Subscription,
    view: View,
    client: Handle,
    composer: Entity<TextField>,
    transcript: Entity<crate::transcript::Transcript>,
    details: Entity<TextView>,
    tab: usize,
    files: Entity<ListModel>,
    changes: std::sync::Arc<Vec<(String, bool, String)>>,
    _draft_subscription: gpui::Subscription,
}
impl Coder {
    fn submit(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text().to_string();
        if text.trim() == "/quit" {
            cx.quit();
            return;
        }
        if !text.trim().is_empty() {
            let _ = self.client.tx.send(Action::Steer(text, self.delivery));
            self.composer.update(cx, |f, cx| f.set_text("", cx));
        }
    }
    fn update_details(&mut self, cx: &mut Context<Self>) {
        let text = match self.tab {
            0 => self.view.work(),
            2 => self.view.session.as_ref().map(|s| serde_json::to_string_pretty(&s.config).unwrap_or_default()).unwrap_or_default(),
            _ => olo_coder_core::commands::HELP.into(),
        };
        if self.details.read(cx).text() != text {
            self.details.update(cx, |d, cx| d.set_text(text, None, cx));
        }
    }
    fn update(&mut self, u: Update, cx: &mut Context<Self>) {
        match &u {
            Update::Opened(id) if id != &self.view.id => {
                if !self.view.id.is_empty() {
                    self.drafts.insert(self.view.id.clone(), self.composer.read(cx).text().into());
                }
                if let Some(text) = self.drafts.remove(id) {
                    self.composer.update(cx, |f, cx| f.set_text(text, cx));
                } else if !self.view.id.is_empty() {
                    self.composer.update(cx, |f, cx| f.set_text("", cx));
                }
            }
            Update::InputResult { session, text, error: Some(_) } => {
                if session == &self.view.id {
                    let existing = self.composer.read(cx).text();
                    let restored = if existing.is_empty() { text.clone() } else { format!("{text}\n\n{existing}") };
                    self.composer.update(cx, |f, cx| f.set_text(restored, cx));
                } else {
                    self.drafts
                        .entry(session.clone())
                        .and_modify(|v| {
                            v.insert_str(0, &format!("{text}\n\n"));
                        })
                        .or_insert(text.clone());
                }
            }
            Update::Catalog { rows, .. } => {
                let selected = self.sessions.read(cx).state.selected.iter().filter_map(|i| self.view.catalog.get(*i).map(|r| r["id"].clone())).collect::<Vec<_>>();
                self.sessions.update(cx, |m, cx| {
                    m.state.selected = rows.iter().enumerate().filter(|(_, r)| selected.contains(&r["id"])).map(|(i, _)| i).collect();
                    cx.notify();
                });
            }
            Update::Session(s) => {
                self.workspace = s.workspace.clone();
                let selected = self.inbox.read(cx).state.selected.iter().filter_map(|i| self.view.session.as_ref()?.inbox.get(*i).map(|p| p.message.id.clone())).collect::<Vec<_>>();
                self.inbox.update(cx, |m, cx| {
                    m.state.selected = s.inbox.iter().enumerate().filter(|(_, p)| selected.contains(&p.message.id)).map(|(i, _)| i).collect();
                    cx.notify();
                });
                if let Some(win) = self.win {
                    let title = format!("{} — Olo Coder", s.title);
                    cx.defer(move |cx| {
                        let _ = win.update(cx, |f, w, _| f.set_title(title, w));
                    });
                }
            }
            _ => {}
        }
        let details = matches!(&u, Update::Session(_) | Update::Opened(_));
        let redraw = matches!(&u, Update::Event(e) if matches!(e.kind.as_str(), "compaction_progress" | "compaction_ready" | "compaction_applied" | "compaction_deferred"))
            || matches!(&u, Update::Rendered(_) | Update::Session(_) | Update::Repository { .. } | Update::Models { .. } | Update::Opened(_) | Update::Catalog { .. } | Update::InputResult { .. });
        let repository = matches!(&u,Update::Repository{session,..} if session==&self.view.id) || matches!(&u, Update::Opened(_));
        self.view.apply(u);
        if repository {
            self.changes = std::sync::Arc::new(
                self.view
                    .repository
                    .as_ref()
                    .and_then(|r| r["files"].as_array())
                    .map(|files| {
                        files
                            .iter()
                            .map(|f| (f["path"].as_str().unwrap_or("").into(), f["staged"] == true, f["state"].as_str().unwrap_or("").replace("Some(", "").trim_end_matches(')').into()))
                            .collect()
                    })
                    .unwrap_or_default(),
            );
            self.files.update(cx, |f, cx| {
                f.state.selected.clear();
                cx.notify();
            });
        }
        let rich = self.view.rich.clone();
        self.transcript.update(cx, |t, cx| t.set(rich, cx));
        if details {
            self.update_details(cx);
        }
        if redraw {
            cx.notify();
        }
    }
    fn prefill(&self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |f, cx| {
            f.set_text(text, cx);
            f.focus(window);
        });
    }
    fn changes_panel(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let rows = self.changes.clone();
        let open_rows = rows.clone();
        let client = self.client.clone();
        let list = ColumnListView::new("coder-changes", &self.files, vec![Column::new("File", 205.0), Column::new("State", 95.0)], rows.len(), move |cell, w, cx| {
            if let Some((path, staged, state)) = rows.get(cell.row) {
                if cell.column == 0 {
                    cell.icon_text(w, cx, "document", "document", path.clone());
                } else {
                    cell.text(w, cx, if *staged { "Staged".to_string() } else { state.clone() });
                }
            }
        })
        .empty_text(self.view.repository.as_ref().and_then(|r| r["error"].as_str()).map(|_| "Open a Git repository to review changes.").unwrap_or("Working tree is clean."))
        .on_open(move |row, _, _| {
            if let Some((path, staged, _)) = open_rows.get(row) {
                client.submit(format!("/git diff {}", serde_json::json!({"path":path,"staged":staged})));
            }
        });
        let selected = self.files.read(cx).state.selected.iter().filter_map(|i| self.changes.get(*i).map(|r| r.0.clone())).collect::<Vec<_>>();
        let stage = self.client.clone();
        let unstage = self.client.clone();
        let refresh = self.client.clone();
        let commit = cx.entity();
        let staged = selected.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(div().flex_1().min_h(px(0.0)).child(list))
            .child(StringView::new("Open a file to review its diff.").dim(true))
            .child(
                div()
                    .flex()
                    .gap(px(m::SPACING))
                    .child(Button::new("Stage").enabled(!selected.is_empty()).on_click(move |_, _| stage.submit(format!("/git stage {}", serde_json::json!({"paths":staged})))))
                    .child(Button::new("Unstage").enabled(!selected.is_empty()).on_click(move |_, _| unstage.submit(format!("/git unstage {}", serde_json::json!({"paths":selected}))))),
            )
            .child(
                div()
                    .flex()
                    .gap(px(m::SPACING))
                    .child(Button::new("Refresh").on_click(move |_, _| {
                        let _ = refresh.tx.send(Action::Repository);
                    }))
                    .child(Button::new("Commit…").on_click(move |w, cx| commit.update(cx, |s, cx| s.command("git_commit", w, cx)))),
            )
            .into_any_element()
    }
    fn settings_panel(&self, inset: gpui::Pixels, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(s) = &self.view.session else { return div().into_any_element() };
        let editable = !matches!(s.status, olo_coder_core::Status::Running | olo_coder_core::Status::Approval);
        let mut panel = div().w_full().flex().flex_col().gap(px(m::TIGHT));
        let provider = cx.entity();
        let approval = self.client.clone();
        let effort = self.client.clone();
        let model = self.client.clone();
        let mut models = self.view.models.clone();
        if !models.contains(&s.config.model.model) {
            models.insert(0, s.config.model.model.clone());
        }
        let selected_model = models.iter().position(|id| id == &s.config.model.model).unwrap_or(0);
        let choices = models.iter().cloned().map(SharedString::from).collect();
        let guidance = cx.entity();
        let advanced = cx.entity();
        panel = panel
            .child(div().ml(-inset).child(StringView::new("Model & reasoning").bold(true)))
            .child(
                MenuField::new("provider", vec!["Codex (Token Pool)".into(), "Custom endpoint".into()], usize::from(s.config.model.provider == olo_coder_core::Provider::Openai))
                    .enabled(editable)
                    .label("Provider:")
                    .divider(80.0)
                    .on_change(move |i, w, cx| {
                        provider.update(cx, |s, cx| {
                            if i == 0 {
                                s.client.submit("/provider codex");
                            } else {
                                s.command("endpoint", w, cx);
                            }
                        })
                    }),
            )
            .child(MenuField::new("model", choices, selected_model).name("Model").enabled(editable).width(280.0).on_change(move |i, _, _| model.submit(format!("/model {}", models[i]))));
        let endpoint = cx.entity();
        panel = panel.child(Button::new("Configure Endpoint…").enabled(editable).on_click(move |w, cx| endpoint.update(cx, |s, cx| s.command("endpoint", w, cx))));
        let efforts = ["", "none", "low", "medium", "high", "xhigh", "max", "ultra"];
        panel = panel
            .child(
                MenuField::new(
                    "reasoning",
                    efforts.iter().map(|s| (if s.is_empty() { "Provider default" } else { *s }).into()).collect(),
                    efforts.iter().position(|v| *v == s.config.model.effort).unwrap_or(0),
                )
                .enabled(editable)
                .label("Reasoning:")
                .divider(80.0)
                .on_change(move |i, _, _| effort.submit(format!("/set model.effort {}", serde_json::json!(efforts[i])))),
            )
            .child(widgets::separator())
            .child(div().ml(-inset).child(StringView::new("Autonomy & approvals").bold(true)))
            .child(
                MenuField::new(
                    "approvals",
                    vec!["Ask".into(), "Automatic review".into(), "Allow".into()],
                    match s.config.approval {
                        olo_coder_core::ApprovalMode::Ask => 0,
                        olo_coder_core::ApprovalMode::Auto => 1,
                        olo_coder_core::ApprovalMode::Allow => 2,
                    },
                )
                .enabled(editable)
                .label("Actions:")
                .divider(80.0)
                .on_change(move |i, _, _| approval.submit(format!("/approval {}", ["ask", "auto", "allow"][i]))),
            )
            .child(Button::new("Approval Guidance…").enabled(editable).on_click(move |w, cx| guidance.update(cx, |s, cx| s.command("guidance", w, cx))));
        for (label, command, checked) in
            [("Keep going toward the goal", "keep", s.config.keep_going), ("Enable subagents", "agents", s.config.subagents), ("Read-only workspace", "readonly", s.config.read_only)]
        {
            let client = self.client.clone();
            panel = panel.child(CheckBox::new(label, checked).enabled(editable).on_toggle(move |on, _, _| client.submit(format!("/{command} {}", if on { "on" } else { "off" }))));
        }
        for (label, key, checked) in [("Accept collaborator messages", "accept_messages", s.config.accept_messages), ("Allow messages to start idle work", "wake_on_message", s.config.wake_on_message)]
        {
            let client = self.client.clone();
            panel = panel.child(CheckBox::new(label, checked).enabled(editable).on_toggle(move |on, _, _| client.submit(format!("/set {key} {on}"))));
        }
        panel.child(div().flex_1()).child(Button::new("All Settings…").enabled(editable).on_click(move |w, cx| advanced.update(cx, |s, cx| s.command("config", w, cx)))).into_any_element()
    }
    fn sessions_panel(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let rows = self.view.catalog.clone();
        let open_rows = rows.clone();
        let open = self.client.clone();
        let list = ColumnListView::new("coder-sessions", &self.sessions, vec![Column::new("Session", 205.0), Column::new("State", 95.0)], rows.len(), move |cell, w, cx| {
            if let Some(row) = rows.get(cell.row) {
                let text = if cell.column == 0 {
                    format!("{}{}", if row["pinned"] == true { "◆ " } else { "" }, row["title"].as_str().unwrap_or("Untitled"))
                } else {
                    format!("{}{}", row["status"].as_str().unwrap_or(""), if row["pending"].as_u64().unwrap_or(0) > 0 { " · mail" } else { "" })
                };
                cell.text(w, cx, text);
            }
        })
        .empty_text("No matching sessions.")
        .on_open(move |row, _, _| {
            if let Some(id) = open_rows.get(row).and_then(|r| r["id"].as_str()) {
                open.submit(format!("/open {id}"));
            }
        });
        let row = self.sessions.read(cx).state.selected.iter().next().and_then(|i| self.view.catalog.get(*i)).cloned();
        let id = row.as_ref().and_then(|r| r["id"].as_str()).map(str::to_owned);
        let enabled = id.is_some();
        let open_id = id.clone();
        let pin_id = id.clone();
        let window_id = id.clone();
        let pin = row.as_ref().is_some_and(|r| r["pinned"] == true);
        let open = self.client.clone();
        let pin_client = self.client.clone();
        let dir = self.dir.clone();
        let workspace = self.workspace.clone();
        let archive = cx.entity();
        let prev = cx.entity();
        let next = cx.entity();
        let next_offset = self.view.next_offset;
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .child(div().px(px(2.0)).child(TextControl::new(self.search.clone())))
            .child(div().flex_1().min_h(px(0.0)).child(list))
            .child(CheckBox::new("Include archived", self.filter.archived).on_toggle(move |on, _, cx| {
                archive.update(cx, |s, cx| {
                    s.filter.archived = on;
                    s.filter.offset = 0;
                    let _ = s.client.tx.send(Action::Browse(s.filter.clone()));
                    cx.notify();
                })
            }))
            .child(
                div()
                    .flex()
                    .gap(px(m::SPACING))
                    .child(Button::new("Open").enabled(enabled).on_click(move |_, _| {
                        if let Some(id) = &open_id {
                            open.submit(format!("/open {id}"));
                        }
                    }))
                    .child(Button::new("New Window").enabled(enabled).on_click(move |_, cx| {
                        let dir = dir.clone();
                        let workspace = workspace.clone();
                        let id = window_id.clone();
                        cx.defer(move |cx| open_coder(cx, dir, workspace, id, serde_json::Value::Null));
                    }))
                    .child(Button::new(if pin { "Unpin" } else { "Pin" }).enabled(enabled).on_click(move |_, _| {
                        if let Some(id) = &pin_id {
                            pin_client.submit(format!("/api {}", serde_json::json!({"op":"organize","session":id,"pinned":!pin})));
                        }
                    })),
            )
            .children((self.filter.offset > 0 || next_offset.is_some()).then(|| {
                div()
                    .flex()
                    .gap(px(m::SPACING))
                    .child(Button::new("Previous").enabled(self.filter.offset > 0).on_click(move |_, cx| {
                        prev.update(cx, |s, _| {
                            s.filter.offset = s.filter.offset.saturating_sub(100);
                            let _ = s.client.tx.send(Action::Browse(s.filter.clone()));
                        })
                    }))
                    .child(Button::new("Next").enabled(next_offset.is_some()).on_click(move |_, cx| {
                        next.update(cx, |s, _| {
                            if let Some(offset) = next_offset {
                                s.filter.offset = offset;
                                let _ = s.client.tx.send(Action::Browse(s.filter.clone()));
                            }
                        })
                    }))
            }))
            .into_any_element()
    }
    fn inbox_panel(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let messages = std::sync::Arc::new(self.view.session.as_ref().map(|s| s.inbox.clone()).unwrap_or_default());
        let rows = messages.clone();
        let list = ColumnListView::new("coder-inbox", &self.inbox, vec![Column::new("Pending message", 205.0), Column::new("Delivery", 95.0)], rows.len(), move |cell, w, cx| {
            if let Some(p) = rows.get(cell.row) {
                cell.text(w, cx, if cell.column == 0 { p.message.text.lines().next().unwrap_or("").to_owned() } else { delivery_name(p.message.delivery).into() });
            }
        })
        .empty_text("No pending messages.");
        let selected = self.inbox.read(cx).state.selected.iter().next().copied();
        let message = selected.and_then(|i| messages.get(i)).map(|p| p.message.clone());
        let editable = message.as_ref().is_some_and(|m| m.delivery != Delivery::Interrupt || self.view.session.as_ref().is_some_and(|s| !s.interrupt_requested));
        let edit_message = message.clone();
        let edit_client = self.client.clone();
        let session = self.view.id.clone();
        let cancel = self.client.clone();
        let cancel_session = session.clone();
        let cancel_id = message.as_ref().map(|m| m.id.clone());
        let up = self.client.clone();
        let up_session = session.clone();
        let up_id = cancel_id.clone();
        let before = selected.and_then(|i| i.checked_sub(1)).and_then(|i| messages.get(i)).map(|p| p.message.id.clone());
        let can_up = before.is_some();
        let retime = self.client.clone();
        let retime_session = session.clone();
        let retime_id = cancel_id.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .child(div().flex_1().min_h(px(0.0)).child(list))
            .child(
                StringView::new(
                    message.as_ref().map(|m| format!("From {}", m.from.as_ref().map(|s| s.title.as_str()).unwrap_or("you"))).unwrap_or_else(|| "Messages are saved before delivery.".into()),
                )
                .dim(true),
            )
            .child(
                MenuField::new("pending-delivery", vec!["After next tool".into(), "Queue after response".into()], usize::from(message.as_ref().is_some_and(|m| m.delivery == Delivery::Queue)))
                    .name("Pending delivery")
                    .enabled(editable)
                    .on_change(move |i, _, _| {
                        if let Some(id) = &retime_id {
                            retime.submit(format!("/api {}", serde_json::json!({"op":"edit_message","session":retime_session,"message":id,"delivery":if i==0{"after_tool"}else{"queue"}})));
                        }
                    }),
            )
            .child(
                div()
                    .flex()
                    .gap(px(m::SPACING))
                    .child(Button::new("Edit…").enabled(editable).on_click(move |_, cx| {
                        if let Some(m) = &edit_message {
                            form::open(
                                cx,
                                "Edit Pending Message",
                                "Changes apply only while this message is still pending.",
                                "Save",
                                vec![Field::new("Message:", &m.text).multiline(8)],
                                edit_client.clone(),
                                form::Submission::Coordination { operation: "edit".into(), session: session.clone(), message: Some(m.id.clone()) },
                            );
                        }
                    }))
                    .child(Button::new("Move Up").enabled(can_up).on_click(move |_, _| {
                        if let Some(id) = &up_id {
                            up.submit(format!("/api {}", serde_json::json!({"op":"move_message","session":up_session,"message":id,"before":before})));
                        }
                    }))
                    .child(Button::new("Cancel").enabled(editable).on_click(move |_, _| {
                        if let Some(id) = &cancel_id {
                            cancel.submit(format!("/api {}", serde_json::json!({"op":"cancel_message","session":cancel_session,"message":id})));
                        }
                    })),
            )
            .into_any_element()
    }
    fn menus(&self) -> Vec<Menu> {
        let running = self.view.session.as_ref().is_some_and(|s| matches!(s.status, olo_coder_core::Status::Running | olo_coder_core::Status::Approval));
        let pending = self.view.session.as_ref().is_some_and(|s| s.pending.is_some());
        let archivable = self.view.session.as_ref().is_some_and(|s| !running && !s.archived && s.calls.is_empty() && s.in_flight.is_none() && s.inbox.is_empty() && s.queued.is_empty());
        vec![
            Menu::new("Session")
                .item("New Window", "new_window")
                .item("New Session", "new")
                .key("n")
                .item("Sessions…", "sessions")
                .key("p")
                .item("Pin Session", "pin")
                .checked(self.view.session.as_ref().is_some_and(|s| s.pinned))
                .item("Rename Session…", "rename")
                .item("Archive Session", "archive")
                .enabled(archivable)
                .item("Restore Session", "restore")
                .enabled(self.view.session.as_ref().is_some_and(|s| s.archived))
                .item("Open Session…", "open")
                .item("Fork Session", "fork")
                .separator()
                .item("Compact Context", "compact")
                .item("Compaction Checkpoints", "compactions")
                .item("Original Transcript", "transcript")
                .separator()
                .item("Close Window", "close")
                .key("w")
                .item("Quit", "quit")
                .key("q"),
            Menu::new("Agent")
                .item("Send Message", "send")
                .key("return")
                .item("Pending Messages", "inbox")
                .item("Message History", "messages")
                .item("Message Another Session…", "message")
                .item("Link Coder Instance…", "connect")
                .item("Linked Instances", "peers")
                .item("Stop", "stop")
                .enabled(running)
                .item("Resume", "resume")
                .enabled(!running && self.view.session.as_ref().is_some_and(|s| !s.archived))
                .separator()
                .item("Start Subagent…", "start_agent")
                .enabled(!running && self.view.session.as_ref().is_some_and(|s| !s.archived && s.config.subagents))
                .item("Set Goal…", "goal")
                .item("Spec Mode", "spec")
                .checked(self.view.session.as_ref().is_some_and(|s| s.spec_mode))
                .item("Accept Specification", "accept")
                .item("Keep Going", "keep")
                .checked(self.view.session.as_ref().is_some_and(|s| s.config.keep_going))
                .separator()
                .item("Allow Action", "approve")
                .enabled(pending)
                .item("Deny Action", "deny")
                .enabled(pending),
            Menu::new("Repository")
                .item("Initialize Repository", "git_init")
                .item("Changes", "changes")
                .key("g")
                .item("Refresh Changes", "git_refresh")
                .item("Commit Staged Changes…", "git_commit")
                .separator()
                .item("History", "git_log")
                .item("Branches", "git_branches")
                .item("Create Branch…", "git_branch")
                .item("Worktrees", "git_worktrees")
                .item("Add Worktree…", "git_worktree")
                .separator()
                .item("GitHub Pull Requests…", "github_pulls")
                .item("Draft Pull Request…", "github_draft"),
            Menu::new("Settings")
                .item("Custom Endpoint…", "endpoint")
                .item("Model…", "model")
                .item("Approval Guidance…", "guidance")
                .item("Configuration…", "config")
                .item("Subagents", "agents")
                .checked(self.view.session.as_ref().is_some_and(|s| s.config.subagents)),
            Menu::new("View")
                .item("Tile Coder Windows", "tile")
                .item("Conversation", "conversation")
                .item("Toggle Follow Conversation", "follow")
                .item("Work", "work")
                .item("Settings", "settings")
                .item("Command Reference", "help")
                .separator()
                .item("Search History…", "history"),
        ]
    }
    fn command(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            "new_window" => {
                let dir = self.dir.clone();
                let workspace = self.workspace.clone();
                cx.defer(move |cx| open_coder(cx, dir, workspace, None, serde_json::Value::Null));
            }
            "tile" => {
                let proxy = olo_ui::proxy(cx);
                std::thread::spawn(move || {
                    if let Err(e) = olo_protocol::Client::connect().and_then(|mut c| {
                        c.request(&olo_protocol::Request::GroupApp { app_id: "olo.coder".into(), grouped: false })?;
                        c.request(&olo_protocol::Request::TileApp { app_id: "olo.coder".into() })
                    }) {
                        proxy.send(WindowNotice(format!("Could not tile windows: {e}")));
                    }
                });
            }
            "conversation" => self.page = 0,
            "messages" => self.client.submit("/inbox"),
            "sessions" | "inbox" => {
                self.tab = if cmd == "sessions" { 3 } else { 4 };
                self.page = self.tab + 1;
            }
            "rename" | "message" | "connect" => {
                let (title, description, action, fields) = match cmd {
                    "rename" => {
                        ("Rename Session", "Give this session a name you can find later.", "Save", vec![Field::new("Name:", self.view.session.as_ref().map(|s| s.title.as_str()).unwrap_or(""))])
                    }
                    "connect" => {
                        ("Link Coder Instance", "Link another local service store; each side keeps its own sessions.", "Link", vec![Field::new("Alias:", ""), Field::new("Store folder:", "")])
                    }
                    _ => (
                        "Message Another Session",
                        "Use a session ID, optionally prefixed with a linked instance alias/.",
                        "Send",
                        vec![Field::new("Recipient:", ""), Field::new("Message:", "").multiline(6)],
                    ),
                };
                form::open(cx, title, description, action, fields, self.client.clone(), form::Submission::Coordination { operation: cmd.into(), session: self.view.id.clone(), message: None });
            }
            "quit" => cx.quit(),
            "close" => window.remove_window(),
            "send" => self.submit(cx),
            "stop" => {
                let _ = self.client.tx.send(Action::Stop);
            }
            "approve" | "deny" => {
                let _ = self.client.tx.send(Action::Approve(cmd == "approve"));
            }
            "follow" => self.transcript.update(cx, |t, cx| {
                t.follow = !t.follow;
                cx.notify();
            }),
            "changes" => {
                self.tab = 1;
                self.page = 2;
                let _ = self.client.tx.send(Action::Repository);
            }
            "start_agent" => {
                form::open(
                    cx,
                    "Start Subagent",
                    "Read-only. Blank fields inherit this session’s settings.",
                    "Start",
                    vec![Field::new("Task:", "").multiline(5), Field::new("Subfolder:", "").optional(), Field::new("Model:", "").optional(), Field::new("Effort:", "").optional()],
                    self.client.clone(),
                    form::Submission::Agent { session: self.view.id.clone() },
                );
            }
            "git_init" => self.client.submit("/git init"),
            "git_refresh" => {
                let _ = self.client.tx.send(Action::Repository);
            }
            "git_log" | "git_branches" | "git_worktrees" => self.client.submit(format!("/git {}", cmd.strip_prefix("git_").unwrap())),
            "git_commit" | "git_branch" | "git_worktree" | "github_draft" => {
                let session = self.view.id.clone();
                let head = self.view.repository.as_ref().and_then(|r| r["head"].as_str()).unwrap_or("").to_owned();
                let repo = self.view.repository.as_ref().and_then(|r| r["github"].as_str()).unwrap_or("").to_owned();
                let branch = self.view.repository.as_ref().and_then(|r| r["branch"].as_str()).unwrap_or("").to_owned();
                let (title, description, action, fields) = match cmd {
                    "git_commit" => ("Commit Staged Changes", "Record the staged changes on the current branch.", "Commit", vec![Field::new("Message:", "").multiline(5)]),
                    "git_branch" => ("Create Branch", "Create a branch at the current commit.", "Create", vec![Field::new("Branch:", "")]),
                    "git_worktree" => ("Add Worktree", "Check out an existing branch in a workspace subfolder.", "Add", vec![Field::new("Branch:", ""), Field::new("Folder:", "")]),
                    _ => (
                        "Draft Pull Request",
                        "Create a draft from a branch already pushed to GitHub.",
                        "Create Draft",
                        vec![
                            Field::new("Repository:", repo),
                            Field::new("Title:", ""),
                            Field::new("Head branch:", branch),
                            Field::new("Base branch:", "main"),
                            Field::new("Description:", "").multiline(6).optional(),
                        ],
                    ),
                };
                form::open(cx, title, description, action, fields, self.client.clone(), form::Submission::Repository { operation: cmd.into(), session, head });
            }
            "github_pulls" => {
                if let Some(repo) = self.view.repository.as_ref().and_then(|r| r["github"].as_str()) {
                    self.client.submit(format!("/github pulls {}", serde_json::json!({"repository":repo})));
                } else {
                    self.prefill("/github pulls {\"repository\":\"owner/repo\"}", window, cx);
                }
            }
            "endpoint" => {
                if let Some(s) = &self.view.session {
                    let config = s.config.clone();
                    let endpoint = config.model.endpoint.clone().unwrap_or_else(|| olo_coder_core::EndpointConfig {
                        base_url: "https://api.fireworks.ai/inference/v1".into(),
                        api_key_env: Some("FIREWORKS_API_KEY".into()),
                        ..Default::default()
                    });
                    let model = if config.model.provider == olo_coder_core::Provider::Openai { config.model.model.clone() } else { String::new() };
                    let effort = if config.model.provider == olo_coder_core::Provider::Openai { config.model.effort.clone() } else { String::new() };
                    form::open(
                        cx,
                        "Custom Model Endpoint",
                        "Use a Chat Completions API. Key names refer to the service environment.",
                        "Save",
                        vec![
                            Field::new("API root:", endpoint.base_url),
                            Field::new("Model ID:", model),
                            Field::new("Key variable:", endpoint.api_key_env.unwrap_or_default()).optional(),
                            Field::new("Effort:", effort).optional(),
                        ],
                        self.client.clone(),
                        form::Submission::Endpoint { session: s.id.clone(), config },
                    );
                }
            }
            "guidance" => {
                let session = self.view.id.clone();
                if let Some(s) = &self.view.session {
                    let config = s.config.clone();
                    form::open(
                        cx,
                        "Approval Guidance",
                        "The automatic reviewer applies this guidance to each action.",
                        "Save",
                        vec![Field::new("Guidance:", &config.approval_guidance).multiline(10).optional()],
                        self.client.clone(),
                        form::Submission::Guidance { session, config },
                    );
                }
            }
            "help" => self.client.submit("/help"),
            "work" | "settings" => {
                self.tab = match cmd {
                    "work" => 0,
                    "settings" => 2,
                    _ => 3,
                };
                self.page = self.tab + 1;
                self.update_details(cx);
            }
            "spec" | "keep" | "agents" => {
                let on = self.view.session.as_ref().is_some_and(|s| match cmd {
                    "spec" => s.spec_mode,
                    "keep" => s.config.keep_going,
                    _ => s.config.subagents,
                });
                self.client.submit(format!("/{cmd} {}", if on { "off" } else { "on" }));
            }
            "new" | "pin" | "archive" | "restore" | "peers" | "fork" | "compact" | "compactions" | "transcript" | "resume" | "accept" => self.client.submit(format!("/{cmd}")),
            "config" => {
                let text = self.view.session.as_ref().map(|s| serde_json::to_string(&s.config).unwrap_or_default()).unwrap_or_default();
                self.prefill(&format!("/config {text}"), window, cx);
            }
            _ => self.prefill(&format!("/{cmd} "), window, cx),
        }
        cx.notify();
    }
}
impl Render for Coder {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menu_entity = cx.entity();
        let bar = MenuBar::new(self.menus(), move |cmd, w, cx| menu_entity.update(cx, |s, cx| s.command(cmd, w, cx)));
        let compact = _window.viewport_size().width < px(950.0);
        // Match the text well's inner border on fractional device scales.
        let well_inset = olo_ui::style::hairline(_window) * 2.0;
        let short = _window.viewport_size().height < px(500.0);
        let tabs_entity = cx.entity();
        let choices: Vec<SharedString> = if compact { vec!["Conversation", "Work", "Changes", "Settings", "Sessions", "Inbox"] } else { vec!["Work", "Changes", "Settings", "Sessions", "Inbox"] }
            .into_iter()
            .map(Into::into)
            .collect();
        let tabs = MenuField::new("coder-panels", choices, if compact { self.page } else { self.tab }).name("Panel").on_change(move |i, _, cx| {
            tabs_entity.update(cx, |s, cx| {
                if compact {
                    s.page = i;
                    if i > 0 {
                        s.tab = i - 1;
                    }
                } else {
                    s.tab = i;
                    s.page = i + 1;
                }
                s.update_details(cx);
                cx.notify();
            })
        });
        let send = cx.entity();
        let stop = self.client.clone();
        let resume = self.client.clone();
        let approve = self.client.clone();
        let deny = self.client.clone();
        let pending = self.view.session.as_ref().is_some_and(|s| s.pending.is_some());
        let running = self.view.session.as_ref().is_some_and(|s| matches!(s.status, olo_coder_core::Status::Running | olo_coder_core::Status::Approval));
        let header = div()
            .flex()
            .items_center()
            .gap(px(m::SPACING))
            .px(px(m::WINDOW_INSET))
            .py(px(m::TIGHT))
            .child(
                canvas(
                    |_, _, _| {},
                    |bounds, _, window, _| {
                        olo_ui::icons::paint(window, bounds, "coder");
                    },
                )
                .w(px(if short { 24.0 } else { 32.0 }))
                .h(px(if short { 24.0 } else { 32.0 }))
                .flex_shrink_0(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .child(StringView::new(self.view.session.as_ref().map(|s| s.title.clone()).unwrap_or_else(|| "Olo Coder".into())).bold(true).cut(true))
                    .children(
                        (!short).then(|| StringView::new(self.view.repository.as_ref().and_then(|r| r["branch"].as_str()).unwrap_or("A persistent coding workspace").to_string()).dim(true).cut(true)),
                    ),
            )
            .child(Button::new("Sessions").on_click({
                let entity = cx.entity();
                move |w, cx| entity.update(cx, |s, cx| s.command("sessions", w, cx))
            }))
            .child(Button::new("Resume").enabled(!running && self.view.session.as_ref().is_some_and(|s| !s.archived)).on_click(move |_, _| resume.submit("/resume")))
            .child(Button::new("Stop").enabled(running).on_click(move |_, _| {
                let _ = stop.tx.send(Action::Stop);
            }));
        let side = match self.tab {
            1 => div().size_full().px(well_inset).child(self.changes_panel(cx)).into_any_element(),
            2 => div().size_full().px(well_inset).child(ScrollView::new("coder-settings", &self.settings_scroll).border(false).child(self.settings_panel(well_inset, cx))).into_any_element(),
            3 => self.sessions_panel(cx),
            4 => self.inbox_panel(cx),
            _ => {
                let mut panel = div().size_full().flex().flex_col().gap(px(m::SPACING));
                if let Some(session) = &self.view.session {
                    let done = session.todos.iter().filter(|t| t.status == "done").count();
                    if !session.todos.is_empty() {
                        panel = panel.child(StatusBar::new(done as f32 / session.todos.len() as f32).label("Plan progress").trailing(format!("{done} / {}", session.todos.len())));
                    }
                }
                panel.child(div().flex_1().min_h(px(0.0)).child(self.details.clone())).into_any_element()
            }
        };
        let main = if compact {
            div()
                .flex_1()
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .gap(px(m::TIGHT))
                .px(px(m::WINDOW_INSET))
                .child(div().flex().justify_end().px(well_inset).child(tabs))
                .child(div().flex_1().min_h(px(0.0)).child(if self.page == 0 { self.transcript.clone().into_any_element() } else { side }))
        } else {
            div().flex_1().min_h(px(0.0)).flex().gap(px(m::SPACING)).px(px(m::WINDOW_INSET)).child(div().flex_1().min_w(px(0.0)).child(self.transcript.clone())).child(
                div().w(px(320.0)).flex_shrink_0().flex().flex_col().gap(px(m::TIGHT)).child(div().flex().justify_end().px(well_inset).child(tabs)).child(div().flex_1().min_h(px(0.0)).child(side)),
            )
        };
        let delivery = cx.entity();
        let mode = MenuField::new(
            "delivery-mode",
            vec!["After next tool".into(), "Queue after response".into(), "Interrupt now".into()],
            match self.delivery {
                Delivery::AfterTool => 0,
                Delivery::Queue => 1,
                Delivery::Interrupt => 2,
            },
        )
        .name("Delivery")
        .on_change(move |i, _, cx| {
            delivery.update(cx, |s, cx| {
                s.delivery = [Delivery::AfterTool, Delivery::Queue, Delivery::Interrupt][i];
                cx.notify();
            })
        });
        // A scale change can force a window below its requested minimum on a
        // small output. Keep the action buttons reachable without truncating
        // the delivery choices or imposing another horizontal scroll area.
        let mode = div().pl(well_inset).child(mode);
        let (mode_above, mode_inline) = if _window.viewport_size().width < px(500.0) { (Some(mode), None) } else { (None, Some(mode)) };
        let composer = div()
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .px(px(m::WINDOW_INSET))
            .py(px(if short { m::TIGHT } else { m::WINDOW_INSET }))
            .child(TextControl::new(self.composer.clone()).label("Message:").divider(72.0).rows(if short {
                1
            } else if compact {
                2
            } else {
                3
            }))
            .children(mode_above)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(m::SPACING))
                    .children(mode_inline)
                    .child(div().flex_1())
                    .child(Button::new("Deny").enabled(pending).on_click(move |_, _| {
                        let _ = deny.tx.send(Action::Approve(false));
                    }))
                    .child(Button::new("Allow Once").enabled(pending).on_click(move |_, _| {
                        let _ = approve.tx.send(Action::Approve(true));
                    }))
                    .child(Button::new("Send").default(true).on_click(move |_, cx| send.update(cx, |s, cx| s.submit(cx)))),
            );
        let status = StringView::new(self.view.status()).dim(true);
        div().size_full().flex().flex_col().child(bar).child(header).child(main).child(composer).child(div().px(px(m::WINDOW_INSET)).pb(px(m::TIGHT)).child(status))
    }
}

fn delivery_name(mode: Delivery) -> &'static str {
    match mode {
        Delivery::AfterTool => "After tool",
        Delivery::Queue => "Queued",
        Delivery::Interrupt => "Interrupt",
    }
}
struct Routed {
    key: usize,
    update: Update,
}
struct WindowNotice(String);
#[derive(Default)]
struct Windows {
    next: usize,
    views: std::collections::BTreeMap<usize, gpui::WeakEntity<Coder>>,
}
impl gpui::Global for Windows {}
impl Drop for Coder {
    fn drop(&mut self) {
        let _ = self.client.tx.send(Action::Quit);
    }
}
fn open_coder(cx: &mut gpui::App, dir: PathBuf, workspace: PathBuf, session: Option<String>, restore: serde_json::Value) {
    let key = cx.update_global::<Windows, _>(|windows, _| {
        windows.next += 1;
        windows.next
    });
    let restore_key = restore["window_key"].as_str().map(str::to_owned).unwrap_or_else(olo_coder_core::id);
    let session = restore["session"].as_str().filter(|s| !s.is_empty()).map(str::to_owned).or(session);
    let proxy = olo_ui::proxy(cx);
    let initial_id = session.clone().unwrap_or_default();
    let client = client::start(dir.clone(), workspace.clone(), session, move |update| proxy.send(Routed { key, update }));
    let restored_client = client.clone();
    let composer = widgets::text_field(cx, |f| f.multiline(true).name("Message").hint("Describe a task, or enter /help"));
    composer.update(cx, |f, cx| f.set_text(restore["draft"].as_str().unwrap_or(""), cx));
    let search = widgets::text_field(cx, |f| f.name("Find sessions").hint("Search titles, tags, or workspaces"));
    let details = widgets::text_view(cx, "coder-work", |v| v.empty_text("Set a goal to track a long-running task."));
    let transcript = crate::transcript::Transcript::new(cx, client.clone());
    transcript.update(cx, |t, _| t.follow = restore["follow"].as_bool().unwrap_or(true));
    let files = ListModel::new(cx, true);
    let sessions = ListModel::new(cx, false);
    let inbox = ListModel::new(cx, false);
    let view = cx.new(|cx| {
        let subscription = cx.subscribe(&composer, |s: &mut Coder, _, e, cx| {
            if matches!(e, TextFieldEvent::Entered) {
                s.submit(cx);
            }
        });
        let search_subscription = cx.subscribe(&search, |s: &mut Coder, f, e, cx| {
            if matches!(e, TextFieldEvent::Changed) {
                s.filter.query = f.read(cx).text().into();
                s.filter.offset = 0;
                let _ = s.client.tx.send(Action::Browse(s.filter.clone()));
            }
        });
        let selection_subscriptions = [&files, &sessions, &inbox].into_iter().map(|m| cx.observe(m, |_, _, cx| cx.notify())).collect();
        Coder {
            restore_key: restore_key.clone(),
            _selection_subscriptions: selection_subscriptions,
            win: None,
            dir,
            workspace,
            drafts: serde_json::from_value(restore["drafts"].clone()).unwrap_or_default(),
            delivery: serde_json::from_value(restore["delivery"].clone()).unwrap_or_default(),
            page: restore["page"].as_u64().unwrap_or(0).min(5) as usize,
            sessions,
            inbox,
            search,
            filter: Default::default(),
            settings_scroll: gpui::ScrollHandle::new(),
            _search_subscription: search_subscription,
            view: View { id: initial_id, ..Default::default() },
            client,
            composer,
            transcript,
            details,
            files,
            changes: Default::default(),
            tab: restore["tab"].as_u64().unwrap_or(0).min(4) as usize,
            _draft_subscription: subscription,
        }
    });
    cx.update_global::<Windows, _>(|windows, _| {
        windows.views.retain(|_, v| v.upgrade().is_some());
        windows.views.insert(key, view.downgrade());
    });
    let opened = view.clone();
    let win = olo_ui::open_window(cx, WindowOptions::new("Olo Coder", 1040.0, 700.0).min_size(500.0, 350.0).restore_key(restore_key), move |w, cx| {
        opened.read(cx).composer.read(cx).focus(w);
        opened.clone()
    });
    view.update(cx, |s, _| s.win = win);
    let forms = serde_json::from_value(restore["forms"].clone()).unwrap_or_default();
    form::restore(cx, forms, restored_client);
}
pub fn run(dir: PathBuf, workspace: PathBuf, session: Option<String>) {
    olo_ui::run("olo.coder", move |cx| {
        cx.set_global(Windows::default());
        olo_ui::message_handler(cx, |message, cx| match message.downcast::<Routed>() {
            Ok(message) => {
                let view = cx.global::<Windows>().views.get(&message.key).and_then(|v| v.upgrade());
                if let Some(view) = view {
                    view.update(cx, |s, cx| s.update(message.update, cx));
                }
            }
            Err(message) => {
                if let Ok(message) = message.downcast::<WindowNotice>() {
                    let views = cx.global::<Windows>().views.values().filter_map(|v| v.upgrade()).collect::<Vec<_>>();
                    for view in views {
                        view.update(cx, |s, cx| s.update(Update::Notice(message.0.clone()), cx));
                    }
                }
            }
        });
        let restore = olo_ui::restored_state().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).unwrap_or_default();
        let states = restore["windows"].as_array().cloned().unwrap_or_else(|| vec![restore]);
        for state in states {
            open_coder(cx, dir.clone(), workspace.clone(), session.clone(), state);
        }
        olo_ui::on_upgrade(cx, |cx| {
            let windows = cx
                .global::<Windows>()
                .views
                .values()
                .filter_map(|v| v.upgrade())
                .map(|v| {
                    let s = v.read(cx);
                    serde_json::json!({"window_key":s.restore_key,"session":s.view.id,"draft":s.composer.read(cx).text(),"drafts":s.drafts,"delivery":s.delivery,"page":s.page,"tab":s.tab,"follow":s.transcript.read(cx).follow})
                })
                .collect::<Vec<_>>();
            let mut windows = windows;
            if let Some(first) = windows.first_mut() {
                first["forms"] = serde_json::to_value(form::snapshots(cx)).unwrap_or_default();
            }
            Some(serde_json::json!({"windows":windows}).to_string())
        });
    });
}
