//! Olo Coder's window, on olo-kit.
//!
//! One window a session, three panes: the sessions on the left (as Tracker
//! lists volumes: pinned first, then by when they were last at work), the
//! conversation in the middle with the message being written under it, and
//! the work on the right (the goal and plan, the repository's changes, the
//! context the model has). An action that needs the person's approval is a
//! banner over the composer with Allow and Deny; nothing else asks.
//!
//! Every window is its own looper with its own connection to the session
//! service (`client`); what the service says arrives as messages, and the
//! conversation's text is made on the presentation worker. Nothing on the
//! looper waits for the service.
pub mod dialog;
pub mod doc;
pub mod settings;

use crate::{
    client::{self, Action, Handle, Update, View as Shown},
    form::{Field, Submission},
};
use olo_coder_core::{ApprovalMode, Delivery, SessionFilter, Status};
use olo_kit::{
    Child, ClickEvent, Cx, Element, FontWeight, IntoElement as _, Menu, MenuBar, ParentElement as _, Pixels, Point, Styled as _, View, WindowOptions, agent, canvas, div, instance, menu, metrics as m,
    px, style, theme,
    widgets::{Button, Column, ColumnListView, EmptyState, ListModel, MenuField, Sidebar, StatusBar, StringView, TabView, TextField, TextFieldEvent, sidebar::Row, text_field},
};
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf, sync::Arc};

const APP_ID: &str = "olo.coder";
/// The sessions' pane and the work's, in points.
const SESSIONS_WIDTH: f32 = 220.0;
const WORK_WIDTH: f32 = 320.0;
/// The composer's field: between these many lines, as tall as what's in it.
const MIN_LINES: usize = 2;
const MAX_LINES: usize = 10;

/// When a message sent while the agent works reaches it.
const DELIVERIES: [(Delivery, &str); 3] = [(Delivery::AfterTool, "After this step"), (Delivery::Queue, "When it finishes"), (Delivery::Interrupt, "Interrupt now")];
const APPROVALS: [(ApprovalMode, &str); 3] = [(ApprovalMode::Ask, "Ask me"), (ApprovalMode::Auto, "Review automatically"), (ApprovalMode::Allow, "Allow everything")];

#[derive(Clone)]
pub enum Msg {
    Update(Update),
    Send,
    Stop,
    Resume,
    Approve(bool),
    Delivery(usize),
    Model(usize),
    Effort(usize),
    Approvals(usize),
    Session(usize),
    SessionMenu(usize, Point<Pixels>),
    Searched,
    Tab(usize),
    Link(String),
    OpenChange(usize),
    Run(String),
    Notice(String),
}

pub struct Coder {
    key: String,
    dir: PathBuf,
    workspace: PathBuf,
    client: Handle,
    view: Shown,
    transcript: Child<doc::Transcript>,
    composer: Child<TextField>,
    search: Child<olo_kit::search::SearchField>,
    /// Drafts of the sessions this window showed before, by session.
    drafts: HashMap<String, String>,
    delivery: Delivery,
    filter: SessionFilter,
    show_sessions: bool,
    show_work: bool,
    tab: usize,
    files: ListModel,
    changes: Arc<Vec<(String, bool, String)>>,
    /// The sessions' rows, each one's session (None: a heading).
    session_rows: Vec<Option<String>>,
    /// What the window last said in its status line, briefly.
    said: Option<String>,
}

impl Coder {
    fn session(&self) -> Option<&olo_coder_core::Session> {
        self.view.session.as_ref()
    }

    fn running(&self) -> bool {
        self.session().is_some_and(|s| matches!(s.status, Status::Running | Status::Approval))
    }

    /// Settings can be changed (the service takes them while it's idle).
    fn settable(&self) -> bool {
        self.session().is_some_and(|s| !matches!(s.status, Status::Running | Status::Approval) && !s.archived)
    }

    fn submit(&self, text: impl Into<String>) {
        self.client.submit(text);
    }

    fn send(&mut self, cx: &mut Cx<Self>) {
        let text = self.composer.read(cx).text().to_owned();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        if trimmed == "/quit" {
            olo_kit::quit();
            return;
        }
        let action = if self.running() && !trimmed.starts_with('/') { Action::Steer(text, self.delivery) } else { Action::Steer(text, Delivery::AfterTool) };
        let _ = self.client.tx.send(action);
        self.composer.update(cx, |f, cx| f.set_text("", cx));
        self.transcript.update(cx, |t, cx| t.to_end(cx));
    }

    fn update(&mut self, u: Update, cx: &mut Cx<Self>) {
        let mut redraw = true;
        match &u {
            Update::Rendered(doc) => {
                let doc = doc.clone();
                self.transcript.update(cx, |t, cx| t.set(doc, cx));
                redraw = false;
            }
            Update::Opened(id) if id != &self.view.id => {
                let draft = self.composer.read(cx).text().to_owned();
                if !self.view.id.is_empty() && !draft.is_empty() {
                    self.drafts.insert(self.view.id.clone(), draft);
                }
                let next = self.drafts.remove(id).unwrap_or_default();
                self.composer.update(cx, |f, cx| f.set_text(next, cx));
            }
            Update::InputResult { session, text, error: Some(error) } => {
                // Not sent: back in the composer (or the draft of the
                // session it was for), with why.
                if session == &self.view.id || session.is_empty() {
                    let existing = self.composer.read(cx).text().to_owned();
                    let restored = if existing.is_empty() { text.clone() } else { format!("{text}\n\n{existing}") };
                    self.composer.update(cx, |f, cx| f.set_text(restored, cx));
                } else {
                    let draft = self.drafts.entry(session.clone()).or_default();
                    *draft = if draft.is_empty() { text.clone() } else { format!("{text}\n\n{draft}") };
                }
                self.said = Some(format!("Not sent: {error}"));
            }
            Update::Session(s) => {
                self.workspace = s.workspace.clone();
                cx.set_window_title(format!("{} — Olo Coder", s.title));
            }
            Update::Repository { session, result } if session == &self.view.id => {
                self.changes = Arc::new(
                    result["files"]
                        .as_array()
                        .map(|files| files.iter().map(|f| (f["path"].as_str().unwrap_or("").into(), f["staged"] == true, f["state"].as_str().unwrap_or("").to_owned())).collect())
                        .unwrap_or_default(),
                );
                self.files.state_mut().selected.clear();
            }
            Update::Event(_) => {
                // (The conversation comes as `Rendered`; what's around it
                // changes with these.)
                let before = (self.view.activity(), self.view.compaction_progress.clone());
                self.view.apply(u);
                if (self.view.activity(), self.view.compaction_progress.clone()) != before {
                    cx.invalidate();
                }
                return;
            }
            Update::Notice(t) if t.starts_with("Error: ") => self.said = Some(t.trim_start_matches("Error: ").to_owned()),
            Update::Profiles(_) | Update::Peers(_) | Update::Catalog { .. } | Update::Models { .. } | Update::Connection(_) => {}
            _ => {}
        }
        self.view.apply(u);
        if redraw {
            cx.invalidate();
        }
    }

    fn menus(&self) -> Vec<Menu> {
        let s = self.session();
        let running = self.running();
        let idle = s.is_some() && !running;
        let pending = s.is_some_and(|s| s.pending.is_some());
        let archived = s.is_some_and(|s| s.archived);
        let pinned = s.is_some_and(|s| s.pinned);
        let spec = s.is_some_and(|s| s.spec_mode);
        let keep = s.is_some_and(|s| s.config.keep_going);
        let agents = s.is_some_and(|s| s.config.subagents);
        let supervisor = s.is_some_and(|s| s.config.supervisor.enabled);
        let read_only = s.is_some_and(|s| s.config.read_only);
        let screens = s.is_some_and(|s| s.config.olo.screens);
        let terminal = s.is_some_and(|s| s.config.olo.terminal);
        let has_spec = s.is_some_and(|s| !s.spec.text.is_empty() && s.spec.accepted_revision != Some(s.spec.revision));
        let follow = self.transcript_follow();
        vec![
            Menu::new("Session")
                .item("New Session", "new")
                .item("New Window", "new_window")
                .item("New Session in Folder…", "new_in")
                .separator()
                .item("Rename…", "rename")
                .enabled(s.is_some())
                .item(if pinned { "Unpin" } else { "Pin" }, "pin")
                .enabled(s.is_some())
                .item("Fork", "fork")
                .enabled(idle)
                .item(if archived { "Unarchive" } else { "Archive" }, if archived { "restore" } else { "archive" })
                .enabled(idle)
                .separator()
                .item("Summarise Context Now", "compact")
                .enabled(idle)
                .item("Checkpoints", "compactions")
                .item("Whole Transcript", "transcript")
                .separator()
                .item("Close Window", "close")
                .item("Quit", "quit"),
            Menu::new("Edit").item("Copy", "copy").item("Select All", "select-all").separator().item("Settings…", "settings"),
            Menu::new("Agent")
                .item("Send", "send")
                .item("Stop", "stop")
                .enabled(running)
                .item("Resume", "resume")
                .enabled(idle && !archived)
                .separator()
                .item("Allow", "approve")
                .enabled(pending)
                .item("Deny", "deny")
                .enabled(pending)
                .separator()
                .item("Set Goal…", "goal")
                .item("Spec Mode", "spec")
                .checked(spec)
                .item("Accept Specification", "accept")
                .enabled(has_spec)
                .item("Keep Going Toward the Goal", "keep")
                .checked(keep)
                .separator()
                .item("Start Subagent…", "start_agent")
                .enabled(agents)
                .item("Allow Subagents", "agents")
                .checked(agents)
                .item("Supervisor", "supervisor_toggle")
                .checked(supervisor)
                .item("Read Only", "readonly")
                .checked(read_only)
                .separator()
                .item("Message Another Session…", "message")
                .item("Link Coder Instance…", "connect"),
            Menu::new("Repository")
                .item("Changes", "changes")
                .item("Refresh", "git_refresh")
                .item("Commit Staged…", "git_commit")
                .separator()
                .item("History", "git_log")
                .item("Branches", "git_branches")
                .item("New Branch…", "git_branch")
                .item("Worktrees", "git_worktrees")
                .item("Add Worktree…", "git_worktree")
                .separator()
                .item("Pull Requests", "github_pulls")
                .item("Draft Pull Request…", "github_draft")
                .separator()
                .item("Make a Repository Here", "git_init"),
            Menu::new("Connections")
                .item("All Connections", "integrations")
                .separator()
                .item("Add Slack…", "setup_slack")
                .item("Add Logfire (US)", "setup_logfire")
                .item("Add Logfire (EU)", "setup_logfire_eu")
                .item("Add MCP Server…", "setup_mcp")
                .item("Manage Connection…", "manage_mcp")
                .separator()
                .item("Agent Screens", "olo_screens")
                .checked(screens)
                .item("Terminal Titles", "olo_terminal")
                .checked(terminal)
                .separator()
                .item("Share a Password…", "share_password")
                .item("Stop Sharing a Password…", "revoke_password"),
            Menu::new("View")
                .item("Sessions", "sidebar")
                .checked(self.show_sessions)
                .item("Work", "toggle_work")
                .checked(self.show_work)
                .separator()
                .item("Plan", "tab_plan")
                .item("Changes", "tab_changes")
                .item("Context", "tab_context")
                .separator()
                .item("Open All Actions", "open_all")
                .item("Close All Actions", "close_all")
                .item("Follow the Conversation", "follow")
                .checked(follow)
                .separator()
                .item("Tile Coder Windows", "tile")
                .item("Commands", "help"),
        ]
    }

    fn transcript_follow(&self) -> bool {
        true
    }

    fn open_dialog(&mut self, title: &str, description: &str, action: &str, fields: Vec<Field>, submission: Submission, cx: &mut Cx<Self>) {
        dialog::open(cx, title, description, action, fields, self.client.clone(), submission);
    }

    fn run_command(&mut self, cmd: &str, cx: &mut Cx<Self>) -> bool {
        let s = self.session().cloned();
        let id = self.view.id.clone();
        match cmd {
            "new" => self.submit("/new"),
            "new_in" => {
                let me = cx.messenger();
                olo_kit::file_panel::show("New Session in Folder", olo_kit::file_panel::FilePanel::folder(self.workspace.clone()), move |answer| {
                    if let olo_kit::file_panel::PanelResult::Chosen(path) = answer.result {
                        me.send(Msg::Run(format!("/new {}", path.display())));
                    }
                });
            }
            "new_window" => open(self.dir.clone(), self.workspace.clone(), None, Value::Null),
            "close" => cx.close_window(),
            "quit" => olo_kit::quit(),
            "copy" => self.transcript.update(cx, |t, cx| t.copy(cx)),
            "select-all" => self.transcript.update(cx, |t, cx| t.select_all(cx)),
            "settings" => settings::open(cx, self.dir.clone(), self.client.clone(), &self.view),
            "send" => self.send(cx),
            "stop" => {
                let _ = self.client.tx.send(Action::Stop);
            }
            "resume" => self.submit("/resume"),
            "approve" | "deny" => {
                let _ = self.client.tx.send(Action::Approve(cmd == "approve"));
            }
            "rename" => {
                let title = s.as_ref().map(|s| s.title.clone()).unwrap_or_default();
                self.open_dialog(
                    "Rename Session",
                    "A name to find it by later.",
                    "Rename",
                    vec![Field::new("Name:", title)],
                    Submission::Coordination { operation: "rename".into(), session: id, message: None, recipients: vec![] },
                    cx,
                );
            }
            "connect" => self.open_dialog(
                "Link Coder Instance",
                "Another Coder service on this computer, by its folder. Each keeps its own sessions; linked, they can message each other.",
                "Link",
                vec![Field::new("Name:", ""), Field::new("Folder:", "")],
                Submission::Coordination { operation: "connect".into(), session: id, message: None, recipients: vec![] },
                cx,
            ),
            "message" => self.open_dialog(
                "Message Another Session",
                "Find the session to write to, on this service or a linked one.",
                "Find",
                vec![
                    Field::new("Service:", "This service").choices(std::iter::once("This service".to_owned()).chain(self.view.peers.iter().map(|p| p.name.clone()))),
                    Field::new("Search:", "").optional(),
                ],
                Submission::Recipient { dir: self.dir.clone(), session: id, peer: None, items: vec![] },
                cx,
            ),
            "goal" => {
                let goal = s.as_ref().and_then(|s| s.goal.as_ref()).map(|g| g.objective.clone()).unwrap_or_default();
                self.open_dialog(
                    "Set Goal",
                    "What the agent works toward until it can show it's done. With Keep Going on, it carries on by itself between your messages.",
                    "Set Goal",
                    vec![Field::new("Goal:", goal).multiline(5)],
                    Submission::Goal { session: id },
                    cx,
                );
            }
            "start_agent" => {
                let models = crate::models::choices(&self.view, None);
                let labels = std::iter::once("Same as this session".to_owned()).chain(models.iter().map(|(label, _)| label.clone())).collect::<Vec<_>>();
                self.open_dialog(
                    "Start Subagent",
                    "A helper with a task of its own. This session’s rules for approval still apply to it.",
                    "Start",
                    vec![
                        Field::new("Task:", "").multiline(5),
                        Field::new("Subfolder:", "").optional(),
                        Field::new("Model:", "Same as this session").choices(labels),
                        Field::new("Reasoning:", "Use model settings").choices(std::iter::once("Use model settings").chain(crate::models::EFFORTS)),
                        Field::new("May:", "Only read").choices(["Only read", "Change files"]),
                    ],
                    Submission::Agent { session: id, models },
                    cx,
                );
            }
            "spec" | "keep" | "agents" | "readonly" => {
                let on = s.as_ref().is_some_and(|s| match cmd {
                    "spec" => s.spec_mode,
                    "keep" => s.config.keep_going,
                    "readonly" => s.config.read_only,
                    _ => s.config.subagents,
                });
                self.submit(format!("/{cmd} {}", if on { "off" } else { "on" }));
            }
            "supervisor_toggle" => self.submit(format!("/supervisor {}", if s.as_ref().is_some_and(|s| s.config.supervisor.enabled) { "off" } else { "on" })),
            "accept" | "fork" | "compact" | "compactions" | "transcript" | "archive" | "restore" | "integrations" => self.submit(format!("/{cmd}")),
            "pin" => self.submit(format!("/api {}", json!({"op":"organize","session":id,"pinned":!s.as_ref().is_some_and(|s| s.pinned)}))),
            "help" => self.submit("/help"),
            "changes" | "tab_changes" => {
                self.show_work = true;
                self.tab = 1;
                let _ = self.client.tx.send(Action::Repository);
            }
            "tab_plan" => {
                self.show_work = true;
                self.tab = 0;
            }
            "tab_context" => {
                self.show_work = true;
                self.tab = 2;
            }
            "sidebar" => self.show_sessions = !self.show_sessions,
            "toggle_work" => self.show_work = !self.show_work,
            "open_all" | "close_all" => {
                let _ = self.client.tx.send(Action::Show(Update::OpenAll(cmd == "open_all")));
            }
            "follow" => self.transcript.update(cx, |t, cx| t.to_end(cx)),
            "tile" => {
                let me = cx.messenger();
                std::thread::spawn(move || {
                    if let Err(e) = olo_protocol::Client::connect().and_then(|mut c| {
                        c.request(&olo_protocol::Request::GroupApp { app_id: APP_ID.into(), grouped: false })?;
                        c.request(&olo_protocol::Request::TileApp { app_id: APP_ID.into() })
                    }) {
                        me.send(Msg::Notice(format!("Couldn’t tile the windows: {e}")));
                    }
                });
            }
            "git_refresh" => {
                let _ = self.client.tx.send(Action::Repository);
            }
            "git_init" => self.submit("/git init"),
            "git_log" | "git_branches" | "git_worktrees" => self.submit(format!("/git {}", cmd.trim_start_matches("git_"))),
            "git_commit" | "git_branch" | "git_worktree" | "github_draft" => {
                let repo = self.view.repository.clone().unwrap_or_default();
                let head = repo["head"].as_str().unwrap_or("").to_owned();
                let github = repo["github"].as_str().unwrap_or("").to_owned();
                let branch = repo["branch"].as_str().unwrap_or("").to_owned();
                let branches: Vec<String> = repo["branches"].as_array().into_iter().flatten().filter_map(|v| v["name"].as_str().map(str::to_owned)).collect();
                let (title, description, action, fields) = match cmd {
                    "git_commit" => ("Commit", "Records what’s staged on the current branch.", "Commit", vec![Field::new("Message:", "").multiline(5)]),
                    "git_branch" => ("New Branch", "A branch at the current commit.", "Make Branch", vec![Field::new("Branch:", "")]),
                    "git_worktree" => (
                        "Add Worktree",
                        "A branch checked out in a folder of its own, inside the workspace.",
                        "Add",
                        vec![Field::new("Branch:", branch.clone()).choices(branches.clone()), Field::new("Folder:", "")],
                    ),
                    _ => (
                        "Draft Pull Request",
                        "A draft on GitHub from a branch that’s been pushed.",
                        "Make Draft",
                        vec![
                            Field::new("Repository:", github),
                            Field::new("Title:", ""),
                            Field::new("From branch:", branch).choices(branches.clone()),
                            Field::new("Into branch:", "main").choices(branches),
                            Field::new("Description:", "").multiline(6).optional(),
                        ],
                    ),
                };
                self.open_dialog(title, description, action, fields, Submission::Repository { operation: cmd.into(), session: id, head }, cx);
            }
            "github_pulls" => match self.view.repository.as_ref().and_then(|r| r["github"].as_str()) {
                Some(repo) => self.submit(format!("/github pulls {}", json!({"repository":repo}))),
                None => self.said = Some("This workspace has no GitHub repository.".into()),
            },
            "olo_screens" | "olo_terminal" => {
                if let Some(s) = &s {
                    let mut config = s.config.olo.clone();
                    if cmd == "olo_screens" {
                        config.screens = !config.screens;
                    } else {
                        config.terminal = !config.terminal;
                    }
                    self.submit(format!("/integrations olo {}", serde_json::to_string(&config).unwrap_or_default()));
                }
            }
            "setup_logfire" | "setup_logfire_eu" => self.submit(format!("/integrations setup {}", json!({"preset":if cmd=="setup_logfire"{"logfire"}else{"logfire-eu"}}))),
            "share_password" => self.open_dialog(
                "Share a Password",
                "Find the item in Passwords, then choose the one field the agent may read. Names are searched; no secret is read until the agent asks for it.",
                "Search",
                vec![Field::new("Search:", "").optional()],
                Submission::Password { dir: self.dir.clone(), session: id, phase: 0, items: vec![] },
                cx,
            ),
            "setup_slack" | "setup_mcp" | "manage_mcp" | "revoke_password" => {
                let (title, description, action, fields, keys) = match cmd {
                    "setup_slack" => (
                        "Add Slack",
                        "Your Slack app’s client ID, and the environment variable its secret is in (the service reads it there). Its redirect address is http://127.0.0.1:8787/callback.",
                        "setup_slack",
                        vec![Field::new("Client ID:", ""), Field::new("Secret variable:", "SLACK_CLIENT_SECRET")],
                        vec!["client_id", "client_secret_env"],
                    ),
                    "setup_mcp" => (
                        "Add MCP Server",
                        "A program to run (with its arguments as JSON), or an HTTPS address. Secrets are named, never typed here: a Passwords reference or an environment variable.",
                        "configure_fields",
                        vec![
                            Field::new("Name:", ""),
                            Field::new("Program or address:", ""),
                            Field::new("Arguments (JSON):", "[]"),
                            Field::new("Sign in with:", "none").choices(["none", "token", "password", "oauth"]),
                            Field::new("Token reference:", "").optional(),
                            Field::new("OAuth client ID:", "").optional(),
                            Field::new("Secret variable:", "").optional(),
                        ],
                        vec!["name", "address", "args", "auth", "reference", "client_id", "client_secret_env"],
                    ),
                    "manage_mcp" => (
                        "Manage Connection",
                        "Test a connection, see what it offers, sign in or out, or remove it.",
                        "manage",
                        vec![
                            Field::new("Connection:", s.as_ref().and_then(|s| s.config.mcp.first()).map(|m| m.name.clone()).unwrap_or_default())
                                .choices(s.iter().flat_map(|s| s.config.mcp.iter().map(|m| m.name.clone()))),
                            Field::new("Do:", "test").choices(["test", "inspect", "login", "logout", "remove"]),
                        ],
                        vec!["name", "action"],
                    ),
                    _ => (
                        "Stop Sharing a Password",
                        "The agent can’t read it again. What it already read stays in the conversation.",
                        "revoke_secret",
                        vec![
                            Field::new("Shared as:", s.as_ref().and_then(|s| s.config.olo.secret_grants.keys().next()).cloned().unwrap_or_default())
                                .choices(s.iter().flat_map(|s| s.config.olo.secret_grants.keys().cloned())),
                        ],
                        vec!["name"],
                    ),
                };
                let fields_title = title;
                self.open_dialog(
                    fields_title,
                    description,
                    "Continue",
                    fields,
                    Submission::Integration { session: id, action: action.into(), keys: keys.into_iter().map(str::to_owned).collect() },
                    cx,
                );
            }
            _ => return false,
        }
        cx.invalidate();
        true
    }

    /// The sessions: the search, pinned ones, the rest by when they last
    /// worked.
    fn sessions(&mut self, cx: &mut Cx<Self>) -> Element {
        let mut rows = Vec::new();
        let mut of = Vec::new();
        let catalog = self.view.catalog.clone();
        let (pinned, rest): (Vec<&Value>, Vec<&Value>) = catalog.iter().partition(|r| r["pinned"] == true);
        let mut chosen = usize::MAX;
        for (heading, list) in [("Pinned", &pinned), ("Sessions", &rest)] {
            if list.is_empty() {
                continue;
            }
            rows.push(Row::heading(heading));
            of.push(None);
            for r in list.iter() {
                let id = r["id"].as_str().unwrap_or("").to_owned();
                let status = r["status"].as_str().unwrap_or("");
                let value = match status {
                    "running" => "Working",
                    "approval" => "Needs you",
                    "failed" => "Error",
                    "paused" => "Paused",
                    _ if r["pending"].as_u64().unwrap_or(0) > 0 => "Mail",
                    _ => "",
                };
                if id == self.view.id {
                    chosen = rows.len();
                }
                let title = r["title"].as_str().filter(|t| !t.is_empty()).unwrap_or("Untitled");
                rows.push(Row::new("coder", title.to_owned()).value(value));
                of.push(Some(id));
            }
        }
        self.session_rows = of;
        let list: Element = if rows.is_empty() {
            EmptyState::new(if self.filter.query.is_empty() { "No sessions yet" } else { "Nothing matches" }).into_any_element()
        } else {
            Sidebar::new("sessions", rows, chosen).on_choose(cx.msg_with(|i: &usize| Msg::Session(*i))).on_menu(cx.msg_with(|(i, at): &(usize, Point<Pixels>)| Msg::SessionMenu(*i, *at))).build(cx)
        };
        div()
            .w(px(SESSIONS_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .child(div().p(px(m::TIGHT)).child(&self.search))
            .child(div().flex_1().min_h_0().child(list))
            .child(div().p(px(m::WINDOW_INSET)).flex().child(Button::new("New Session").on_click(cx.msg(Msg::Run("new".into())))))
            .into_any_element()
    }

    /// Who the session is and what it's doing: its name, where it works,
    /// and Resume when it stopped.
    fn header(&self, cx: &mut Cx<Self>) -> Element {
        let s = self.session();
        let title = s.map(|s| s.title.clone()).unwrap_or_else(|| "Olo Coder".into());
        let branch = self.view.repository.as_ref().and_then(|r| r["branch"].as_str()).map(|b| format!(" · {b}")).unwrap_or_default();
        let place = s.map(|s| format!("{}{branch}", tilde(&s.workspace))).unwrap_or_default();
        let running = self.running();
        let activity = self.view.activity();
        // 1: waiting for the person; 2: stopped by an error.
        let tone: u8 = match s.map(|s| &s.status) {
            Some(Status::Approval) => 1,
            Some(Status::Failed) => 2,
            _ => 0,
        };
        // Waiting for the person isn't working: the mark says so, still.
        let busy = tone != 1 && (running || self.view.connection.is_some() || self.view.compaction_progress.is_some());
        let mark = canvas(move |g, b| {
            let p = theme::palette();
            let c = b.center();
            if busy {
                style::spinner(g, c, 7.0, style::c(p.keyboard_navigation));
                g.paint_again_within(1.0 / 30.0);
            } else {
                let color = match tone {
                    1 => p.warning,
                    2 => p.failure,
                    _ => p.success,
                };
                let r = style::snap_centred(g, c, 7.0, 7.0);
                g.paint_quad(olo_kit::fill(r, style::c(color)).corner_radii(px(3.5)));
            }
        })
        .paint_key(style::key((busy, tone)))
        .w(px(16.0))
        .h(px(16.0))
        .flex_shrink_0();
        let resumable = s.is_some_and(|s| !s.archived && matches!(s.status, Status::Paused | Status::Interrupted | Status::Failed));
        // Stop is beside the composer, where the person is while it works.
        let button = if running {
            None
        } else if resumable {
            Some(Button::new("Resume").on_click(cx.msg(Msg::Resume)))
        } else {
            None
        };
        div()
            .flex()
            .items_center()
            .gap(px(m::SPACING))
            .px(px(m::WINDOW_INSET))
            .py(px(m::TIGHT))
            .flex_shrink_0()
            .child(div().flex_1().min_w_0().flex().flex_col().child(StringView::new(title).bold(true).cut(true)).child(StringView::new(place).dim(true).small(true).cut(true)))
            .child(div().flex().items_center().gap(px(m::TIGHT)).flex_shrink_0().child(mark).child(StringView::new(activity).dim(!running)))
            .children(button)
            .into_any_element()
    }

    /// An action waiting for the person: what it is and why, Allow, Deny.
    fn approval(&self, cx: &mut Cx<Self>) -> Option<Element> {
        let a = self.session()?.pending.as_ref()?;
        let what = describe(&a.call);
        let reason = if a.reason.trim().is_empty() { "It isn’t allowed without asking.".to_owned() } else { a.reason.clone() };
        let title = format!("Allow {what}?");
        let (name, value) = (title.clone(), reason.clone());
        let back = canvas(move |g, b| {
            let p = theme::palette();
            let ink = style::c(p.warning);
            style::fill_rect(g, b, ink.opacity(0.14));
            style::frame_rect(g, b, ink);
            g.agent(agent::Node { role: "Status", name: Some(name.clone().into()), value: Some(value.clone().into()), bounds: b, enabled: true, ..Default::default() });
        })
        .paint_key(style::key((&title, &reason)))
        .absolute()
        .inset_0();
        let mark = canvas(|g, b| {
            let p = theme::palette();
            let r = style::snap_centred(g, b.center(), 18.0, 18.0);
            g.paint_quad(olo_kit::fill(r, style::c(p.warning)).corner_radii(px(9.0)));
            style::label(g, b.center().x, b.center().y, style::Align::Center, "!", style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), style::ink_on(p.warning));
        })
        .paint_key(0)
        .w(px(20.0))
        .h(px(20.0))
        .flex_shrink_0();
        let row = div()
            .relative()
            .flex()
            .items_center()
            .gap(px(m::SPACING))
            .px(px(m::SPACING))
            .py(px(m::TIGHT))
            .child(back)
            .child(mark)
            .child(div().flex_1().min_w_0().flex().flex_col().child(StringView::new(title).bold(true).wrap(true)).child(StringView::new(reason).wrap(true).dim(true)))
            .child(div().flex().gap(px(m::TIGHT)).flex_shrink_0().child(Button::new("Deny").on_click(cx.msg(Msg::Approve(false)))).child(Button::new("Allow").on_click(cx.msg(Msg::Approve(true)))));
        Some(div().px(px(m::WINDOW_INSET)).pt(px(m::TIGHT)).child(row).into_any_element())
    }

    /// Messages waiting for the agent to reach them.
    fn queue(&self, cx: &mut Cx<Self>) -> Option<Element> {
        let s = self.session()?;
        if s.inbox.is_empty() {
            return None;
        }
        let mut col = div().px(px(m::WINDOW_INSET)).pt(px(m::TIGHT)).flex().flex_col().gap(px(2.0));
        for p in s.inbox.iter().take(4) {
            let when = DELIVERIES.iter().find(|(d, _)| *d == p.message.delivery).map_or("", |(_, l)| l);
            let from = p.message.from.as_ref().map(|f| format!("{}: ", f.title)).unwrap_or_default();
            let first = p.message.text.lines().next().unwrap_or("");
            let id = p.message.id.clone();
            let session = s.id.clone();
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(m::TIGHT))
                    .child(div().flex_1().min_w_0().child(StringView::new(format!("Waiting · {when} — {from}{first}")).dim(true).cut(true)))
                    .child(Button::new("Cancel").small().on_click(cx.msg(Msg::Run(format!("/api {}", json!({"op":"cancel_message","session":session,"message":id})))))),
            );
        }
        if s.inbox.len() > 4 {
            col = col.child(StringView::new(format!("and {} more", s.inbox.len() - 4)).dim(true).small(true));
        }
        Some(col.into_any_element())
    }

    /// The message being written, and how it goes: the model, its
    /// reasoning, what may be done without asking; Send.
    fn composer(&self, width: f32, cx: &mut Cx<Self>) -> Element {
        let s = self.session();
        let settable = self.settable();
        let running = self.running();
        let lines = self.composer.read(cx).rows_at(px((width - 2.0 * m::WINDOW_INSET - 2.0 * (2.0 + m::TIGHT)).max(80.0))).clamp(MIN_LINES, MAX_LINES);
        let lh = f32::from(TextField::line_height_for(cx.scale_factor(), style::PLAIN_SIZE));
        let field_h = lines as f32 * lh + 2.0 * (2.0 + m::TIGHT);
        let focused = self.composer.read(cx).is_focused();
        let well = canvas(move |g, b| {
            let p = theme::palette();
            style::fill_rect(g, style::inset(g, b, 2.0), style::c(p.document_background));
            style::sunken(g, b, &p, focused);
        })
        .paint_key(style::key(focused))
        .absolute()
        .inset_0();
        let field = div().relative().h(px(field_h)).flex_shrink_0().child(well).child(div().absolute().inset(px(2.0 + m::TIGHT)).child(&self.composer));
        // The model: this session's, the ones its provider offers, the
        // saved profiles.
        let mut models: Vec<String> = self.view.models.clone();
        let current = s.map(|s| s.config.model.model.clone()).unwrap_or_default();
        if !current.is_empty() && !models.contains(&current) {
            models.insert(0, current.clone());
        }
        let model_at = models.iter().position(|m| *m == current).unwrap_or(0);
        let mut model_labels: Vec<olo_kit::SharedString> = models.iter().map(|m| m.clone().into()).collect();
        for p in &self.view.profiles {
            model_labels.push(format!("{} (saved)", p.name).into());
        }
        if model_labels.is_empty() {
            model_labels.push("Model".into());
        }
        let effort = s.map(|s| crate::models::effort(&s.config.model.effort)).unwrap_or(0);
        let approval = s.map(|s| APPROVALS.iter().position(|(a, _)| *a == s.config.approval).unwrap_or(0)).unwrap_or(0);
        let mut controls = div()
            .flex()
            .items_center()
            .gap(px(m::TIGHT))
            .child(MenuField::new("model", model_labels, model_at).name("Model").width(190.0).enabled(settable).on_change(cx.msg_with(|i: &usize| Msg::Model(*i))))
            .child(
                MenuField::new("effort", crate::models::EFFORTS.iter().map(|e| (if *e == "Provider default" { "Reasoning: default" } else { e }).to_owned().into()).collect(), effort)
                    .name("Reasoning")
                    .width(140.0)
                    .enabled(settable)
                    .on_change(cx.msg_with(|i: &usize| Msg::Effort(*i))),
            )
            .child(
                MenuField::new("approvals", APPROVALS.iter().map(|(_, l)| (*l).into()).collect(), approval)
                    .name("Approvals")
                    .width(170.0)
                    .enabled(settable)
                    .on_change(cx.msg_with(|i: &usize| Msg::Approvals(*i))),
            );
        if running {
            let at = DELIVERIES.iter().position(|(d, _)| *d == self.delivery).unwrap_or(0);
            controls = controls
                .child(MenuField::new("delivery", DELIVERIES.iter().map(|(_, l)| (*l).into()).collect(), at).name("Deliver").width(150.0).on_change(cx.msg_with(|i: &usize| Msg::Delivery(*i))));
        }
        controls = controls.child(div().flex_1());
        if running {
            controls = controls.child(Button::new("Stop").on_click(cx.msg(Msg::Stop)));
        }
        let has_text = !self.composer.read(cx).text().trim().is_empty();
        controls = controls.child(Button::new(if running { "Steer" } else { "Send" }).default(true).enabled(has_text && s.is_some_and(|s| !s.archived)).on_click(cx.msg(Msg::Send)));
        div().flex().flex_col().gap(px(m::TIGHT)).px(px(m::WINDOW_INSET)).py(px(m::TIGHT)).flex_shrink_0().child(field).child(controls).into_any_element()
    }

    /// The work: the goal and plan, the repository's changes, the context.
    fn work(&mut self, cx: &mut Cx<Self>) -> Element {
        let tabs: Vec<olo_kit::SharedString> =
            vec!["Plan".into(), format!("Changes{}", if self.changes.is_empty() { String::new() } else { format!(" ({})", self.changes.len()) }).into(), "Context".into()];
        let content = match self.tab {
            1 => self.changes_pane(cx),
            2 => self.context_pane(),
            _ => self.plan_pane(),
        };
        div().w(px(WORK_WIDTH)).h_full().flex_shrink_0().p(px(m::TIGHT)).child(TabView::new("work", tabs, self.tab).on_select(cx.msg_with(|i: &usize| Msg::Tab(*i))).child(content)).into_any_element()
    }

    fn plan_pane(&self) -> Element {
        let Some(s) = self.session() else { return div().into_any_element() };
        if s.goal.is_none() && s.todos.is_empty() && s.spec.text.is_empty() {
            return div()
                .flex()
                .flex_col()
                .gap(px(m::TIGHT))
                .child(StringView::new("No plan yet").bold(true))
                .child(StringView::new("The agent writes its plan here as it works. For work that spans days, set a goal: Agent ▸ Set Goal.").wrap(true).dim(true))
                .into_any_element();
        }
        let mut col = div().flex().flex_col().gap(px(m::TIGHT));
        if let Some(g) = &s.goal {
            col = col.child(StringView::new("Goal").bold(true)).child(StringView::new(g.objective.clone()).wrap(true));
            let status = match g.status.as_str() {
                "complete" | "completed" | "done" => "Reached".to_owned(),
                "active" | "" => "Working toward it".to_owned(),
                other => other.to_owned(),
            };
            col = col.child(StringView::new(status).dim(true).small(true));
        }
        if !s.todos.is_empty() {
            let done = s.todos.iter().filter(|t| t.status == "done").count();
            col = col.child(div().h(px(m::TIGHT))).child(StatusBar::new(done as f32 / s.todos.len() as f32).label("Plan").trailing(format!("{done} of {}", s.todos.len())));
            for t in &s.todos {
                let mark = match t.status.as_str() {
                    "done" => "☑",
                    "in_progress" => "▸",
                    _ => "☐",
                };
                col = col.child(StringView::new(format!("{mark}  {}", t.text)).wrap(true).dim(t.status == "done"));
            }
        }
        if !s.spec.text.is_empty() {
            let accepted = s.spec.accepted_revision == Some(s.spec.revision);
            col = col
                .child(div().h(px(m::TIGHT)))
                .child(StringView::new(format!("Specification · revision {}", s.spec.revision)).bold(true))
                .child(StringView::new(if accepted { "Accepted" } else { "Not accepted yet (Agent ▸ Accept Specification)" }).dim(true).small(true));
            for c in &s.spec.criteria {
                let met = matches!(c.status.as_str(), "done" | "met" | "passed");
                col = col.child(StringView::new(format!("{}  {}", if met { "☑" } else { "☐" }, c.text)).wrap(true));
            }
        }
        olo_kit::widgets::ScrollView::new(&olo_kit::ScrollHandle::new()).border(false).auto_hide(true).child(col.p(px(m::TIGHT))).into_any_element()
    }

    fn changes_pane(&mut self, cx: &mut Cx<Self>) -> Element {
        let error = self.view.repository.as_ref().and_then(|r| r["error"].as_str()).is_some();
        let rows = self.changes.clone();
        let list = ColumnListView::new(cx, "changes", &self.files, vec![Column::new("File", 200.0), Column::new("", 70.0)], rows.len(), move |_: &Coder, cell, g| {
            if let Some((path, staged, state)) = rows.get(cell.row) {
                if cell.column == 0 {
                    cell.icon_text(g, "document", "document", path.clone());
                } else {
                    cell.text(g, if *staged { "staged".to_string() } else { state.clone() });
                }
            }
        })
        .empty_text(if error { "Not a Git repository." } else { "Nothing changed." })
        .on_open(cx.msg_with(|row: &usize| Msg::OpenChange(*row)));
        let selected: Vec<String> = self.files.state().selected.iter().filter_map(|i| self.changes.get(*i).map(|r| r.0.clone())).collect();
        let any = !selected.is_empty();
        let stage = format!("/git stage {}", json!({"paths":selected}));
        let unstage = format!("/git unstage {}", json!({"paths":selected}));
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .child(div().flex_1().min_h_0().child(list))
            .child(
                div()
                    .flex()
                    .gap(px(m::TIGHT))
                    .child(Button::new("Stage").small().enabled(any).on_click(cx.msg(Msg::Run(stage))))
                    .child(Button::new("Unstage").small().enabled(any).on_click(cx.msg(Msg::Run(unstage))))
                    .child(div().flex_1())
                    .child(Button::new("Commit…").small().enabled(!self.changes.is_empty()).on_click(cx.msg(Msg::Run("git_commit".into())))),
            )
            .into_any_element()
    }

    fn context_pane(&self) -> Element {
        let Some(s) = self.session() else { return div().into_any_element() };
        let window = s.config.model.context_window;
        let used = s.usage.context;
        let fraction = if window > 0 { (used as f32 / window as f32).min(1.0) } else { 0.0 };
        let n = crate::feed::thousands;
        let mut col = div().flex().flex_col().gap(px(m::TIGHT)).p(px(m::TIGHT));
        col = col.child(StatusBar::new(fraction).label("In context").trailing(if window > 0 { format!("{} of {}", n(used), n(window)) } else { n(used) }));
        let at = if s.config.model.compact_at > 0 { s.config.model.compact_at } else { window * 8 / 10 };
        col = col.child(
            StringView::new(if s.config.model.auto_compact {
                format!("Earlier work is summarised at {} tokens; the whole conversation is kept.", n(at))
            } else {
                "Summarising is off: Settings ▸ Context.".into()
            })
            .wrap(true)
            .dim(true)
            .small(true),
        );
        col = col
            .child(div().h(px(m::TIGHT)))
            .child(StringView::new("Used so far").bold(true))
            .child(StringView::new(format!("{} in · {} out", n(s.usage.input), n(s.usage.output))))
            .child(StringView::new(format!("{} read from cache · {} requests", n(s.usage.cached), n(s.usage.requests))).dim(true))
            .child(StringView::new(format!("{} turns · {} checkpoints", s.turns, s.compactions)).dim(true));
        let budget = s.config.token_budget;
        if budget > 0 {
            let spent = s.usage.input + s.usage.output;
            col = col.child(div().h(px(m::TIGHT))).child(StatusBar::new((spent as f32 / budget as f32).min(1.0)).label("Budget").trailing(format!("{} of {}", n(spent), n(budget))));
        }
        if !s.memory.trim().is_empty() {
            col = col.child(div().h(px(m::TIGHT))).child(StringView::new("Remembered").bold(true)).child(StringView::new(s.memory.trim().to_owned()).wrap(true).dim(true));
        }
        col = col.child(div().h(px(m::TIGHT))).child(StringView::new(format!("Session {}", s.id)).dim(true).small(true).cut(true));
        olo_kit::widgets::ScrollView::new(&olo_kit::ScrollHandle::new()).border(false).auto_hide(true).child(col).into_any_element()
    }

    fn session_menu(&mut self, row: usize, at: Point<Pixels>, cx: &mut Cx<Self>) {
        let Some(Some(id)) = self.session_rows.get(row).cloned() else { return };
        let r = self.view.catalog.iter().find(|r| r["id"].as_str() == Some(&id)).cloned().unwrap_or_default();
        let pinned = r["pinned"] == true;
        let menu = Menu::new("Session")
            .item("Open", "open")
            .item("Open in New Window", "window")
            .separator()
            .item(if pinned { "Unpin" } else { "Pin" }, "pin")
            .item(if r["archived"] == true { "Unarchive" } else { "Archive" }, "archive");
        let me = cx.messenger();
        let archived = r["archived"] == true;
        menu::context_menu(cx, menu, at, move |cmd, _| {
            let run = match cmd {
                "open" => format!("/open {id}"),
                "window" => format!("@window {id}"),
                "pin" => format!("/api {}", json!({"op":"organize","session":id,"pinned":!pinned})),
                "archive" => format!("/api {}", json!({"op":"organize","session":id,"archived":!archived})),
                _ => return,
            };
            me.send(Msg::Run(run));
        });
    }
}

impl View for Coder {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Update(u) => return self.update(u, cx),
            Msg::Send => self.send(cx),
            Msg::Stop => {
                let _ = self.client.tx.send(Action::Stop);
            }
            Msg::Resume => self.submit("/resume"),
            Msg::Approve(allow) => {
                let _ = self.client.tx.send(Action::Approve(allow));
            }
            Msg::Delivery(i) => self.delivery = DELIVERIES[i.min(2)].0,
            Msg::Model(i) => {
                let mut models: Vec<String> = self.view.models.clone();
                let current = self.session().map(|s| s.config.model.model.clone()).unwrap_or_default();
                if !current.is_empty() && !models.contains(&current) {
                    models.insert(0, current);
                }
                if let Some(model) = models.get(i) {
                    self.submit(format!("/model {model}"));
                } else if let Some(p) = self.view.profiles.get(i - models.len()) {
                    self.submit(format!("/profiles apply {}", json!({"id":p.id,"role":"primary"})));
                }
            }
            Msg::Effort(i) => self.submit(format!("/set model.effort {}", json!(crate::models::effort_value(i)))),
            Msg::Approvals(i) => self.submit(format!("/approval {}", ["ask", "auto", "allow"][i.min(2)])),
            Msg::Session(i) => {
                if let Some(Some(id)) = self.session_rows.get(i) {
                    if *id != self.view.id {
                        self.submit(format!("/open {id}"));
                    }
                }
            }
            Msg::SessionMenu(i, at) => return self.session_menu(i, at, cx),
            Msg::Searched => {
                self.filter.query = self.search.read(cx).text(cx).to_owned();
                self.filter.offset = 0;
                let _ = self.client.tx.send(Action::Browse(self.filter.clone()));
                return;
            }
            Msg::Tab(i) => {
                self.tab = i;
                if i == 1 {
                    let _ = self.client.tx.send(Action::Repository);
                }
            }
            Msg::Link(link) => {
                if let Some(id) = link.strip_prefix("olo-coder:toggle/").and_then(|v| v.parse().ok()) {
                    self.transcript.update(cx, |t, _| t.follow = false);
                    let _ = self.client.tx.send(Action::Show(Update::Toggle(id)));
                } else if let Some(command) = client::read_link(&link) {
                    self.submit(command);
                } else if link.starts_with("https://") || link.starts_with("http://") {
                    std::thread::spawn(move || {
                        let _ = olo_xdg::open::open(&olo_xdg::open::Target::parse(&link));
                    });
                }
                return;
            }
            Msg::OpenChange(row) => {
                if let Some((path, staged, _)) = self.changes.get(row) {
                    self.submit(format!("/git diff {}", json!({"path":path,"staged":staged})));
                }
                return;
            }
            Msg::Run(command) => {
                if let Some(id) = command.strip_prefix("@window ") {
                    open(self.dir.clone(), self.workspace.clone(), Some(id.to_owned()), Value::Null);
                } else if command.starts_with('/') {
                    self.submit(command);
                } else {
                    self.run_command(&command, cx);
                }
                return;
            }
            Msg::Notice(text) => self.said = Some(text),
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        self.run_command(command, cx)
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let width = f32::from(cx.viewport_size().width);
        // Narrow, the panes give way to the conversation.
        let sessions = self.show_sessions && width >= 760.0;
        let work = self.show_work && width >= if sessions { 1040.0 } else { 820.0 };
        let center = width - if sessions { SESSIONS_WIDTH + 1.0 } else { 0.0 } - if work { WORK_WIDTH + 1.0 } else { 0.0 };
        let bar = MenuBar::new(cx, self.menus());
        let scale = cx.scale_factor();
        let (rule, vrule) = (|| rule(scale), || vrule(scale));
        let mut middle = div().flex_1().min_w_0().h_full().flex().flex_col().child(self.header(cx)).child(rule()).child(div().flex_1().min_h_0().child(&self.transcript));
        if let Some(banner) = self.approval(cx) {
            middle = middle.child(banner);
        }
        if let Some(queue) = self.queue(cx) {
            middle = middle.child(queue);
        }
        middle = middle.child(self.composer(center, cx));
        let mut panes = div().flex_1().min_h_0().flex();
        if sessions {
            panes = panes.child(self.sessions(cx)).child(vrule());
        }
        panes = panes.child(middle);
        if work {
            panes = panes.child(vrule()).child(self.work(cx));
        }
        let status = self.said.clone().unwrap_or_else(|| self.view.status());
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(bar)
            .child(panes)
            .child(rule())
            .child(div().px(px(m::WINDOW_INSET)).py(px(m::TIGHT)).pb(px(m::WINDOW_INSET)).flex_shrink_0().child(StringView::new(status).small(true).dim(true).cut(true)))
            .into_any_element()
    }

    fn save_state(&mut self, cx: &mut Cx<Self>) -> Option<String> {
        let mut drafts = self.drafts.clone();
        let draft = self.composer.read(cx).text().to_owned();
        if !draft.is_empty() {
            drafts.insert(self.view.id.clone(), draft);
        }
        Some(
            json!({
                "window_key": self.key,
                "session": self.view.id,
                "drafts": drafts,
                "delivery": self.delivery,
                "tab": self.tab,
                "sessions": self.show_sessions,
                "work": self.show_work,
            })
            .to_string(),
        )
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        let _ = self.client.tx.send(Action::Quit);
    }
}

/// A line between panes, one device pixel thick (so at 1.5× what's
/// beside it starts on the next pixel, not half a pixel on).
fn rule(scale: f32) -> Element {
    canvas(|g, b| style::hline(g, b.left(), b.right(), b.top(), style::c(theme::palette().control_border))).paint_key(0).h(px(1.0 / scale)).w_full().flex_shrink_0().into_any_element()
}

fn vrule(scale: f32) -> Element {
    canvas(|g, b| style::vline(g, b.left(), b.top(), b.bottom(), style::c(theme::palette().control_border))).paint_key(0).w(px(1.0 / scale)).h_full().flex_shrink_0().into_any_element()
}

/// A folder as the person reads it (their home as ~).
fn tilde(path: &std::path::Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    match path.strip_prefix(&home) {
        Ok(rest) if !home.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

/// An action, in a few words (for asking whether it may run).
fn describe(call: &olo_coder_core::ToolCall) -> String {
    let a = &call.arguments;
    match call.name.as_str() {
        "execute" => {
            let args: Vec<&str> = a["args"].as_array().map(|v| v.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            let quoted = |s: &str| if s.is_empty() || s.contains(char::is_whitespace) { format!("'{s}'") } else { s.to_owned() };
            let line = std::iter::once(a["program"].as_str().unwrap_or("").to_owned()).chain(args.into_iter().map(quoted)).collect::<Vec<_>>().join(" ");
            format!("running “{}”", olo_coder_core::tools::clip(&line, 120))
        }
        "write_file" => format!("writing {}", a["path"].as_str().unwrap_or("a file")),
        "edit_file" => format!("editing {}", a["path"].as_str().unwrap_or("a file")),
        "delete_file" => format!("deleting {}", a["path"].as_str().unwrap_or("a file")),
        "fetch_url" => format!("fetching {}", a["url"].as_str().unwrap_or("a page")),
        name => name.replace("__", ": ").replace('_', " "),
    }
}

/// Opens a window showing `session` (a new one in `workspace` if None).
pub fn open(dir: PathBuf, workspace: PathBuf, session: Option<String>, restore: Value) {
    let key = restore["window_key"].as_str().map(str::to_owned).unwrap_or_else(olo_coder_core::id);
    let session = restore["session"].as_str().filter(|s| !s.is_empty()).map(str::to_owned).or(session);
    let options = WindowOptions::new("Olo Coder", 1180.0, 760.0).min_size(520.0, 420.0).restore_key(key.clone()).keep_size("main");
    olo_kit::open_window(options, move |cx| {
        let messenger = cx.messenger();
        let initial = session.clone().unwrap_or_default();
        let client = client::start(dir.clone(), workspace.clone(), session, move |u| {
            messenger.send(Msg::Update(u));
        });
        let transcript = cx.new_view(|_| doc::Transcript::new(true));
        cx.subscribe(&transcript, |_: &mut Coder, e: &doc::Event, cx| match e {
            doc::Event::Link(link) => cx.send(Msg::Link(link.clone())),
        });
        let drafts: HashMap<String, String> = serde_json::from_value(restore["drafts"].clone()).unwrap_or_default();
        let draft = drafts.get(&initial).cloned().unwrap_or_default();
        let composer = text_field(cx, |f| f.multiline(true).name("Message").hint("Ask, or describe a task. ⇧↩ for a new line.").with_text(draft));
        cx.subscribe(&composer, |me: &mut Coder, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Entered => me.send(cx),
            // (Its height follows its lines, and Send is enabled with text.)
            TextFieldEvent::Changed => cx.invalidate(),
            _ => {}
        });
        let search = olo_kit::search::search_field(cx, "Find Sessions");
        cx.subscribe(&search, |_: &mut Coder, e: &olo_kit::search::SearchEvent, cx| {
            if matches!(e, olo_kit::search::SearchEvent::Changed) {
                cx.send(Msg::Searched);
            }
        });
        composer.focus(cx);
        let mut drafts = drafts;
        drafts.remove(&initial);
        Coder {
            key,
            dir,
            workspace,
            client,
            view: Shown { id: initial, feed_events: false, ..Default::default() },
            transcript,
            composer,
            search,
            drafts,
            delivery: serde_json::from_value(restore["delivery"].clone()).unwrap_or_default(),
            filter: SessionFilter::default(),
            show_sessions: restore["sessions"].as_bool().unwrap_or(true),
            show_work: restore["work"].as_bool().unwrap_or(true),
            tab: restore["tab"].as_u64().unwrap_or(0).min(2) as usize,
            files: ListModel::new(true),
            changes: Arc::default(),
            session_rows: vec![],
            said: None,
        }
    });
}

/// How the app was launched: the folder and session asked for.
fn launch_args(args: &[String], cwd: Option<PathBuf>) -> (PathBuf, Option<String>) {
    let mut workspace = cwd.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let mut session = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--workspace" => workspace = args.get(i + 1).map(PathBuf::from).unwrap_or(workspace),
            "--session" => session = args.get(i + 1).cloned(),
            _ => {}
        }
        i += 1;
    }
    (workspace, session)
}

pub fn run(dir: PathBuf, workspace: PathBuf, session: Option<String>) {
    let mut args = vec!["--workspace".to_owned(), workspace.display().to_string()];
    if let Some(s) = &session {
        args.extend(["--session".to_owned(), s.clone()]);
    }
    // One Coder a person: a second launch opens its window in the first.
    if std::env::var_os("OLO_CODER_HOME").is_none() && instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, move |app| {
        instance::listen(APP_ID, olo_kit::proxy());
        let opener = dir.clone();
        olo_kit::message_handler(app, move |message, _| {
            if let Ok(launch) = message.downcast::<instance::Launch>() {
                let (workspace, session) = launch_args(&launch.args, launch.cwd.clone());
                open(opener.clone(), workspace, session, Value::Null);
            }
        });
        // Each window keeps its own; together they're the app's.
        olo_kit::on_upgrade(|states| {
            let windows: Vec<Value> = states.iter().filter_map(|s| serde_json::from_str(s).ok()).collect();
            Some(json!({ "windows": windows }).to_string())
        });
        let restored = olo_kit::restored_state().and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or_default();
        let windows = match restored["windows"].as_array() {
            Some(w) if !w.is_empty() => w.clone(),
            _ if restored.is_object() => vec![restored],
            _ => vec![Value::Null],
        };
        for state in windows {
            open(dir.clone(), workspace.clone(), session.clone(), state);
        }
    });
}

#[allow(dead_code)]
fn _unused(_: ClickEvent, _: FontWeight, _: agent::Node) {}
