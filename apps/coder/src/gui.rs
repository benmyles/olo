use crate::client::{self, Action, Handle, Update, View};
use crate::form::{self, Field};
use olo_ui::{
    Menu, MenuBar, WindowOptions,
    gpui::{self, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, canvas, div, px},
    metrics as m,
    widgets::{self, Button, CheckBox, Column, ColumnListView, ListModel, MenuField, StatusBar, StringView, TabView, TextControl, TextField, TextFieldEvent, TextView},
};
use std::{path::PathBuf, rc::Rc};

struct Coder {
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
            self.client.submit(text);
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
        let details = matches!(&u, Update::Session(_) | Update::Opened(_));
        let redraw = matches!(&u, Update::Rendered(_) | Update::Session(_) | Update::Repository { .. } | Update::Models { .. } | Update::Opened(_));
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
    fn settings_panel(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(s) = &self.view.session else { return div().into_any_element() };
        let editable = !matches!(s.status, olo_coder_core::Status::Running | olo_coder_core::Status::Approval);
        let mut panel = div().size_full().flex().flex_col().gap(px(m::SPACING));
        let provider = self.client.clone();
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
            .child(StringView::new("Model & reasoning").bold(true))
            .child(
                MenuField::new("provider", vec!["Codex".into(), "Claude".into()], usize::from(s.config.model.provider == olo_coder_core::Provider::Claude))
                    .enabled(editable)
                    .label("Provider:")
                    .divider(80.0)
                    .on_change(move |i, _, _| provider.submit(format!("/provider {}", if i == 0 { "codex" } else { "claude" }))),
            )
            .child(MenuField::new("model", choices, selected_model).name("Model").enabled(editable).width(280.0).on_change(move |i, _, _| model.submit(format!("/model {}", models[i]))));
        let efforts = ["none", "low", "medium", "high", "xhigh", "max", "ultra"];
        panel = panel
            .child(
                MenuField::new("reasoning", efforts.iter().map(|s| (*s).into()).collect(), efforts.iter().position(|v| *v == s.config.model.effort).unwrap_or(2))
                    .enabled(editable)
                    .label("Reasoning:")
                    .divider(80.0)
                    .on_change(move |i, _, _| effort.submit(format!("/set model.effort {}", serde_json::json!(efforts[i])))),
            )
            .child(widgets::separator())
            .child(StringView::new("Autonomy & approvals").bold(true))
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
        panel.child(div().flex_1()).child(Button::new("All Settings…").enabled(editable).on_click(move |w, cx| advanced.update(cx, |s, cx| s.command("config", w, cx)))).into_any_element()
    }
    fn menus(&self) -> Vec<Menu> {
        vec![
            Menu::new("Session")
                .item("New Session", "new")
                .key("n")
                .item("Sessions…", "sessions")
                .item("Open Session…", "open")
                .item("Fork Session", "fork")
                .separator()
                .item("Compact Context", "compact")
                .separator()
                .item("Close Window", "close")
                .key("w")
                .item("Quit", "quit")
                .key("q"),
            Menu::new("Agent")
                .item("Send Message", "send")
                .key("return")
                .item("Stop", "stop")
                .item("Resume", "resume")
                .separator()
                .item("Set Goal…", "goal")
                .item("Spec Mode", "spec")
                .checked(self.view.session.as_ref().is_some_and(|s| s.spec_mode))
                .item("Accept Specification", "accept")
                .item("Keep Going", "keep")
                .checked(self.view.session.as_ref().is_some_and(|s| s.config.keep_going))
                .separator()
                .item("Allow Action", "approve")
                .item("Deny Action", "deny"),
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
                .item("Model…", "model")
                .item("Approval Guidance…", "guidance")
                .item("Configuration…", "config")
                .item("Subagents", "agents")
                .checked(self.view.session.as_ref().is_some_and(|s| s.config.subagents)),
            Menu::new("View")
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
                let _ = self.client.tx.send(Action::Repository);
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
            "new" | "sessions" | "fork" | "compact" | "resume" | "accept" => self.client.submit(format!("/{cmd}")),
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
        let tabs_entity = cx.entity();
        let tabs = TabView::new("coder-detail-tabs", Rc::<[SharedString]>::from(vec!["Work".into(), "Changes".into(), "Settings".into()]), self.tab).on_select(move |i, _, cx| {
            tabs_entity.update(cx, |s, cx| {
                s.tab = i;
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
                .w(px(32.0))
                .h(px(32.0)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(StringView::new(self.view.session.as_ref().and_then(|s| s.workspace.file_name()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "Olo Coder".into())).bold(true))
                    .child(StringView::new(self.view.repository.as_ref().and_then(|r| r["branch"].as_str()).unwrap_or("A persistent coding workspace").to_string()).dim(true)),
            )
            .child(div().flex_1())
            .child(Button::new("Resume").enabled(!running).on_click(move |_, _| resume.submit("/resume")))
            .child(Button::new("Stop").enabled(running).on_click(move |_, _| {
                let _ = stop.tx.send(Action::Stop);
            }));
        let side = match self.tab {
            1 => self.changes_panel(cx),
            2 => self.settings_panel(cx),
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
        let main = div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .gap(px(m::SPACING))
            .px(px(m::WINDOW_INSET))
            .child(div().flex_1().min_w(px(0.0)).child(self.transcript.clone()))
            .child(div().w(px(320.0)).flex_shrink_0().child(tabs.child(side)));
        let composer = div().flex().flex_col().gap(px(m::TIGHT)).p(px(m::WINDOW_INSET)).child(TextControl::new(self.composer.clone()).label("Message:").divider(72.0).rows(3)).child(
            div()
                .flex()
                .items_center()
                .gap(px(m::SPACING))
                .child(StringView::new("Enter sends · Shift+Enter newline · /help").dim(true))
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
pub fn run(dir: PathBuf, workspace: PathBuf, session: Option<String>) {
    olo_ui::run("olo.coder", move |cx| {
        let restore = olo_ui::restored_state().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        let session = restore.as_ref().and_then(|v| v["session"].as_str()).map(str::to_owned).or(session);
        let draft = restore.as_ref().and_then(|v| v["draft"].as_str()).unwrap_or("").to_string();
        let tab = restore.as_ref().and_then(|v| v["tab"].as_u64()).unwrap_or(0).min(2) as usize;
        let follow = restore.as_ref().and_then(|v| v["follow"].as_bool()).unwrap_or(true);
        let forms = restore.as_ref().and_then(|v| serde_json::from_value::<Vec<form::Snapshot>>(v["forms"].clone()).ok()).unwrap_or_default();
        let proxy = olo_ui::proxy(cx);
        let client = client::start(dir, workspace, session, move |u| proxy.send(u));
        let restored_client = client.clone();
        let composer = widgets::text_field(cx, |f| f.multiline(true).name("Message").hint("Describe a task, or enter /help"));
        composer.update(cx, |f, cx| f.set_text(draft, cx));
        let details = widgets::text_view(cx, "coder-work", |v| v.empty_text("Set a goal to track a long-running task."));
        let transcript = crate::transcript::Transcript::new(cx, client.clone());
        transcript.update(cx, |t, _| t.follow = follow);
        let files = ListModel::new(cx, true);
        let view = cx.new(|cx| {
            let subscription = cx.subscribe(&composer, |s: &mut Coder, _, event, cx| {
                if matches!(event, TextFieldEvent::Entered) {
                    s.submit(cx);
                }
            });
            Coder { view: View::default(), client, composer, transcript, details, files, changes: std::sync::Arc::default(), tab, _draft_subscription: subscription }
        });
        let opened = view.clone();
        olo_ui::open_window(cx, WindowOptions::new("Olo Coder", 1120.0, 700.0).min_size(900.0, 660.0), move |window, cx| {
            let field = opened.read(cx).composer.clone();
            field.update(cx, |f, _| f.focus(window));
            opened.clone()
        });
        form::restore(cx, forms, restored_client);
        let messages = view.clone();
        olo_ui::message_handler(cx, move |message, cx| {
            if let Ok(update) = message.downcast::<Update>() {
                messages.update(cx, |s, cx| s.update(*update, cx));
            }
        });
        olo_ui::on_upgrade(cx, move |cx| {
            let s = view.read(cx);
            Some(serde_json::json!({"session":s.view.id,"draft":s.composer.read(cx).text(),"tab":s.tab,"follow":s.transcript.read(cx).follow,"forms":form::snapshots(cx)}).to_string())
        });
    });
}
