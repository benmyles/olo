//! The Agent Settings conflict review window's state (`WinRole::Carry`):
//! compares this computer's carried agent settings (`olo_rex_client::carry`)
//! against a far host's account by item (JSON keys, MCP servers, Codex TOML
//! settings, Markdown instruction sections, and skill/command files), lets
//! the person choose strategies in one batch (Copy, Skip, or Merge where
//! allowed), tracks which conflicts have been decided, remembers choices
//! per host in desktop settings, and exposes the full diff and actions to
//! agents via Rex commands.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use olo_protocol::{RexCarryChoice as Choice, RexCarryRemembered as Remembered};
use olo_rex_client::carry::{DiffItem, DiffKind, Plan};
use olo_sync::Mutex;
use serde::{Deserialize, Serialize};

use crate::hub::RexHub;

/// State of the Agent Settings review window for one host at a time.
#[derive(Clone, Debug, Default)]
pub struct Shown {
    /// Host key in `RexHub`.
    pub key: String,
    /// Host display name.
    pub name: String,
    /// Active tab: 0 = Changes, 1 = Remembered.
    pub tab: usize,
    /// Inspecting the host over SSH.
    pub checking: bool,
    /// Applying selected choices over SSH.
    pub applying: bool,
    /// Last error from checking or applying.
    pub error: Option<String>,
    /// Confirmation note after a successful apply.
    pub applied_note: Option<String>,
    /// Latest computed plan from the host.
    pub plan: Option<Plan>,
    /// Ordered pending item ids (Conflicts first, then New, then Remembered updates).
    pub order: Vec<String>,
    /// Per-item resolution chosen in the UI, keyed by `DiffItem::id`.
    pub choices: HashMap<String, Choice>,
    /// Conflict item ids whose action has been explicitly decided by the user
    /// (or covered by a remembered choice). Untouched conflict suggestions render
    /// in secondary ink until decided.
    pub decided: HashSet<String>,
    /// Per-item "Remember" flag toggled in the UI, keyed by `DiffItem::id`.
    pub remember: HashMap<String, bool>,
    /// All remembered choices stored for this host.
    pub remembered: Vec<Remembered>,
    /// Preview mode: false = Both Versions (side-by-side aligned diff),
    /// true = Result on host (under chosen action with provenance tints).
    pub preview_result: bool,
    /// Optional demo initial selection for headless visual QA (row indices).
    pub demo_selection: Option<Vec<usize>>,
}

impl Shown {
    /// Pending items in display order (Conflicts first, then New, then Remembered updates).
    pub fn ordered_pending(&self) -> Vec<&DiffItem> {
        let Some(plan) = &self.plan else {
            return Vec::new();
        };
        let by_id: HashMap<&str, &DiffItem> = plan.items.iter().filter(|it| !it.resolved_same).map(|it| (it.id.as_str(), it)).collect();
        self.order.iter().filter_map(|id| by_id.get(id.as_str()).copied()).collect()
    }

    pub fn choice_of(&self, item: &DiffItem) -> Choice {
        let raw = self.choices.get(&item.id).copied().unwrap_or_else(|| item.interactive_default());
        if raw == Choice::Merge && !item.can_merge { Choice::TakeDesktop } else { raw }
    }

    pub fn is_decided(&self, item: &DiffItem) -> bool {
        item.kind != DiffKind::Conflict || item.remembered.is_some() || self.decided.contains(&item.id)
    }

    pub fn remember_of(&self, id: &str) -> bool {
        self.remember.get(id).copied().unwrap_or(false)
    }

    /// `(total_unremembered_conflicts, decided_unremembered_conflicts)`.
    pub fn conflict_progress(&self) -> (usize, usize) {
        let mut total = 0usize;
        let mut done = 0usize;
        for it in self.ordered_pending() {
            if it.kind == DiffKind::Conflict && it.remembered.is_none() {
                total += 1;
                if self.decided.contains(&it.id) {
                    done += 1;
                }
            }
        }
        (total, done)
    }

    /// How many files will actually be written if Apply is clicked right now.
    pub fn files_to_write(&self) -> usize {
        let Some(plan) = &self.plan else {
            return 0;
        };
        plan.apply(&self.choices).len()
    }

    /// How many remembered-choice updates (new, changed, or unchecked) Apply will persist.
    pub fn remember_edits(&self) -> Vec<(String, Option<Choice>)> {
        let Some(plan) = &self.plan else {
            return Vec::new();
        };
        let existing: HashMap<&str, Choice> = self.remembered.iter().map(|r| (r.item.as_str(), r.choice)).collect();
        let mut edits = Vec::new();
        for item in plan.pending_items() {
            let want_rem = self.remember_of(&item.id);
            let ch = self.choice_of(item);
            match (existing.get(item.id.as_str()).copied(), want_rem) {
                (None, true) => edits.push((item.id.clone(), Some(ch))),
                (Some(old), true) if old != ch => edits.push((item.id.clone(), Some(ch))),
                (Some(_), false) => edits.push((item.id.clone(), None)),
                _ => {}
            }
        }
        edits
    }

    /// Whether Apply has work to do (writing files or saving remembered choices).
    pub fn can_apply(&self) -> bool {
        !self.checking && !self.applying && self.plan.is_some() && (self.files_to_write() > 0 || !self.remember_edits().is_empty())
    }

    /// Structured JSON snapshot exposed on the Carry window's agent node so
    /// agents can read every difference, preview, and remembered choice.
    pub fn agent_json(&self) -> String {
        #[derive(Serialize)]
        struct AgentItem<'a> {
            id: &'a str,
            file: &'a str,
            item: &'a str,
            kind: DiffKind,
            can_merge: bool,
            choice: Choice,
            decided: bool,
            remember: bool,
            remembered: Option<Choice>,
            here: &'a str,
            there: Option<&'a str>,
            merged_preview: Option<&'a str>,
        }
        #[derive(Serialize)]
        struct AgentRemembered<'a> {
            item: &'a str,
            choice: Choice,
        }
        #[derive(Serialize)]
        struct AgentSnapshot<'a> {
            host: &'a str,
            host_name: &'a str,
            checking: bool,
            applying: bool,
            error: Option<&'a str>,
            applied_note: Option<&'a str>,
            same_files: usize,
            files_to_write: usize,
            items: Vec<AgentItem<'a>>,
            remembered: Vec<AgentRemembered<'a>>,
        }
        let pending = self.ordered_pending();
        let items = pending
            .iter()
            .map(|it| AgentItem {
                id: &it.id,
                file: &it.file,
                item: &it.item,
                kind: it.kind,
                can_merge: it.can_merge,
                choice: self.choice_of(it),
                decided: self.is_decided(it),
                remember: self.remember_of(&it.id),
                remembered: it.remembered,
                here: &it.here,
                there: it.there.as_deref(),
                merged_preview: it.merged_preview.as_deref(),
            })
            .collect();
        let remembered = self.remembered.iter().map(|r| AgentRemembered { item: &r.item, choice: r.choice }).collect();
        let snap = AgentSnapshot {
            host: &self.key,
            host_name: &self.name,
            checking: self.checking,
            applying: self.applying,
            error: self.error.as_deref(),
            applied_note: self.applied_note.as_deref(),
            same_files: self.plan.as_ref().map_or(0, |p| p.same_files),
            files_to_write: self.files_to_write(),
            items,
            remembered,
        };
        serde_json::to_string(&snap).unwrap_or_default()
    }
}

/// JSON payload accepted by `host.carry.choose:KEY:{JSON}` and `host.carry.apply:KEY:{JSON}`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AgentChoiceBatch {
    #[serde(default)]
    pub items: Vec<AgentChoiceSpec>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AgentChoiceSpec {
    pub id: String,
    pub choice: Choice,
    #[serde(default)]
    pub remember: Option<bool>,
}

static LIVE: Mutex<Option<Shown>> = Mutex::new(None);

fn live() -> olo_sync::MutexGuard<'static, Option<Shown>> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn current() -> Option<Shown> {
    live().clone()
}

/// Consume the one-shot demo initial selection (used by `ui.rs` on first render).
pub fn take_demo_selection() -> Option<Vec<usize>> {
    live().as_mut().and_then(|s| s.demo_selection.take())
}

/// Order pending items: unremembered conflicts first, then new items, then remembered updates.
fn ordered_ids(plan: &Plan) -> Vec<String> {
    let mut items: Vec<&DiffItem> = plan.pending_items();
    items.sort_by(|a, b| {
        let rank = |it: &DiffItem| match (it.remembered.is_some(), it.kind) {
            (false, DiffKind::Conflict) => 0u8,
            (false, DiffKind::New) => 1u8,
            (true, _) => 2u8,
        };
        rank(a).cmp(&rank(b)).then_with(|| a.file.cmp(&b.file)).then_with(|| a.item.cmp(&b.item))
    });
    items.into_iter().map(|it| it.id.clone()).collect()
}

fn install_plan(shown: &mut Shown, plan: Plan, remembered: Vec<Remembered>) {
    shown.order = ordered_ids(&plan);
    shown.choices.clear();
    shown.decided.clear();
    shown.remember.clear();
    for item in plan.pending_items() {
        shown.choices.insert(item.id.clone(), item.interactive_default());
        shown.remember.insert(item.id.clone(), item.remembered.is_some());
        if item.remembered.is_some() {
            shown.decided.insert(item.id.clone());
        }
    }
    shown.remembered = remembered;
    shown.plan = Some(plan);
    shown.checking = false;
    shown.error = None;
}

/// Open the Agent Settings review window for host `key` (`name`) and inspect it.
pub fn open(hub: &Arc<RexHub>, key: &str, name: &str) {
    if crate::server::demo_mode() {
        *live() = Some(demo_shown(key, name));
        crate::ui::carry_changed();
        return;
    }
    let remembered = hub.carry_choices_for(key);
    {
        let mut g = live();
        *g = Some(Shown { key: key.to_owned(), name: name.to_owned(), checking: true, remembered, ..Shown::default() });
    }
    crate::ui::carry_changed();
    check(hub);
}

pub fn close() {
    let mut g = live();
    if g.as_ref().is_some_and(|s| !s.applying) {
        *g = None;
    }
}

pub fn set_tab(tab: usize) {
    if let Some(s) = live().as_mut() {
        s.tab = tab.min(1);
    }
    crate::ui::carry_changed();
}

pub fn set_preview_result(result: bool) {
    if let Some(s) = live().as_mut() {
        s.preview_result = result;
    }
    crate::ui::carry_changed();
}

/// Apply `choice` to every item id in `ids` (skipping `Merge` for items that cannot merge)
/// and mark each touched conflict as decided.
pub fn set_choices_for(ids: &[String], choice: Choice) {
    let mut g = live();
    let Some(shown) = g.as_mut() else { return };
    let Some(plan) = &shown.plan else { return };
    for id in ids {
        if let Some(item) = plan.items.iter().find(|it| it.id == *id) {
            if choice == Choice::Merge && !item.can_merge {
                continue;
            }
            shown.choices.insert(id.clone(), choice);
            shown.decided.insert(id.clone());
        }
    }
    shown.applied_note = None;
    drop(g);
    crate::ui::carry_changed();
}

/// Set the `Remember` toggle for every item id in `ids`.
pub fn set_remember_for(ids: &[String], remember: bool) {
    let mut g = live();
    let Some(shown) = g.as_mut() else { return };
    for id in ids {
        shown.remember.insert(id.clone(), remember);
        if remember {
            shown.decided.insert(id.clone());
        }
    }
    shown.applied_note = None;
    drop(g);
    crate::ui::carry_changed();
}

/// Apply a structured agent choice batch to the current open Carry state.
pub fn apply_agent_batch(batch: &AgentChoiceBatch) {
    let mut g = live();
    let Some(shown) = g.as_mut() else { return };
    let Some(plan) = &shown.plan else { return };
    for spec in &batch.items {
        if let Some(item) = plan.items.iter().find(|it| it.id == spec.id) {
            let ch = if spec.choice == Choice::Merge && !item.can_merge { Choice::TakeDesktop } else { spec.choice };
            shown.choices.insert(spec.id.clone(), ch);
            shown.decided.insert(spec.id.clone());
            if let Some(rem) = spec.remember {
                shown.remember.insert(spec.id.clone(), rem);
            }
        }
    }
    shown.applied_note = None;
    drop(g);
    crate::ui::carry_changed();
}

/// Re-inspect the current host's agent settings differences over SSH.
pub fn check(hub: &Arc<RexHub>) {
    if crate::server::demo_mode() {
        return;
    }
    let key = {
        let mut g = live();
        let Some(shown) = g.as_mut() else { return };
        if shown.applying {
            return;
        }
        shown.checking = true;
        shown.error = None;
        shown.key.clone()
    };
    crate::ui::carry_changed();
    let (of, hub_cb) = (key.clone(), hub.clone());
    hub.inspect_carry_plan(&key, move |res| {
        let remembered = hub_cb.carry_choices_for(&of);
        let mut g = live();
        if let Some(shown) = g.as_mut().filter(|s| s.key == of) {
            match res {
                Ok(plan) => install_plan(shown, plan, remembered),
                Err(why) => {
                    shown.checking = false;
                    shown.error = Some(why);
                }
            }
        }
        drop(g);
        crate::ui::carry_changed();
    });
}

/// Apply the current choices (and persist Remember toggles) to the host.
pub fn apply(hub: &Arc<RexHub>) {
    if crate::server::demo_mode() {
        let mut g = live();
        if let Some(shown) = g.as_mut() {
            shown.applied_note = Some("Updated 4 files  ·  Remembered 1 new choice".into());
        }
        drop(g);
        crate::ui::carry_changed();
        return;
    }
    let (key, plan, choices, edits) = {
        let mut g = live();
        let Some(shown) = g.as_mut() else { return };
        if shown.checking || shown.applying {
            return;
        }
        let Some(plan) = shown.plan.clone() else { return };
        let edits = shown.remember_edits();
        let files = plan.apply(&shown.choices);
        if files.is_empty() && edits.is_empty() {
            return;
        }
        shown.applying = true;
        shown.error = None;
        shown.applied_note = None;
        (shown.key.clone(), plan, shown.choices.clone(), edits)
    };
    crate::ui::carry_changed();

    // Persist Remember edits first so the subsequent re-check honors them immediately.
    let saved_count = edits.iter().filter(|(_, v)| v.is_some()).count();
    if !edits.is_empty() {
        hub.update_carry_choices(&key, edits);
    }
    let to_write = plan.apply(&choices);
    let written_files = to_write.len();
    let (of, hub_cb) = (key.clone(), hub.clone());
    hub.apply_carry_files(&key, to_write, move |res| {
        match res {
            Ok(()) => {
                let note = match (written_files, saved_count) {
                    (0, n) if n > 0 => olo_kit::words::count(n as u64, "Remembered 1 new choice", "Remembered {n} new choices"),
                    (f, 0) => olo_kit::words::count(f as u64, "Updated 1 file", "Updated {n} files"),
                    (f, n) => format!(
                        "{}  ·  {}",
                        olo_kit::words::count(f as u64, "Updated 1 file", "Updated {n} files"),
                        olo_kit::words::count(n as u64, "Remembered 1 new choice", "Remembered {n} new choices")
                    ),
                };
                if let Some(shown) = live().as_mut().filter(|s| s.key == of) {
                    shown.applying = false;
                    shown.applied_note = Some(note);
                }
                crate::ui::carry_changed();
                // Re-inspect so both Carry and Account windows reflect the new state.
                check(&hub_cb);
            }
            Err(why) => {
                if let Some(shown) = live().as_mut().filter(|s| s.key == of) {
                    shown.applying = false;
                    shown.error = Some(why);
                }
                crate::ui::carry_changed();
            }
        }
    });
}

/// Forget selected remembered items (or all if `items` is `None`) for the
/// window's host (or explicit `host_key`), then refresh.
pub fn forget(hub: &Arc<RexHub>, host_key: Option<&str>, items: Option<&[String]>) {
    let key = match host_key {
        Some(k) => k.to_owned(),
        None => match live().as_ref() {
            Some(s) => s.key.clone(),
            None => return,
        },
    };
    if crate::server::demo_mode() {
        let mut g = live();
        if let Some(shown) = g.as_mut().filter(|s| s.key == key) {
            match items {
                Some(ids) => shown.remembered.retain(|r| !ids.contains(&r.item)),
                None => shown.remembered.clear(),
            }
        }
        drop(g);
        crate::ui::carry_changed();
        return;
    }
    hub.forget_carry_choices(&key, items);
    let updated = hub.carry_choices_for(&key);
    let is_open = {
        let mut g = live();
        if let Some(shown) = g.as_mut().filter(|s| s.key == key) {
            shown.remembered = updated;
            shown.applied_note = None;
            true
        } else {
            false
        }
    };
    hub.ask_account(&key, crate::ui::account_changed);
    if is_open {
        check(hub);
    } else {
        crate::ui::carry_changed();
    }
}

/// Sync remembered list if desktop settings changed externally while open.
pub fn sync_remembered(hub: &RexHub) {
    let mut g = live();
    let Some(shown) = g.as_mut() else { return };
    if crate::server::demo_mode() {
        return;
    }
    let latest = hub.carry_choices_for(&shown.key);
    if shown.remembered != latest {
        shown.remembered = latest;
        drop(g);
        crate::ui::carry_changed();
    }
}

/// Demo state for headless visual QA (`OLO_REX_DEMO_CARRY=1|multi|new-multi|json|long|tall|merged|remembered|empty`).
/// All states share one internally consistent dataset matching Account (`4 conflicts · 2 new`, 6 same, 3 remembered -> 4 remembered after Apply updating 4 files).
pub fn demo_shown(key: &str, name: &str) -> Shown {
    let mode = std::env::var("OLO_REX_DEMO_CARRY").unwrap_or_default();
    let remembered = vec![
        Remembered { host: key.to_owned(), item: ".claude/settings.json::theme".into(), choice: Choice::KeepHost },
        Remembered { host: key.to_owned(), item: ".claude/settings.json::permissions.allow".into(), choice: Choice::Merge },
        Remembered { host: key.to_owned(), item: ".codex/config.toml::model".into(), choice: Choice::TakeDesktop },
    ];
    if mode == "empty" {
        let mut after_rem = remembered;
        after_rem.push(Remembered { host: key.to_owned(), item: ".claude/commands/ship.md".into(), choice: Choice::KeepHost });
        return Shown {
            key: key.to_owned(),
            name: name.to_owned(),
            plan: Some(Plan { home: "/home/you".into(), same_files: 11, remembered_total: after_rem.len(), ..Plan::default() }),
            remembered: after_rem,
            applied_note: Some("Updated 4 files  ·  Remembered 1 new choice".into()),
            ..Shown::default()
        };
    }
    let long_style_here = if mode == "long" {
        "## Style\n- Keep functions small and readable across every module in this workspace.\n- Build Rust crates with jato (never raw cargo when jato is available on the host, so every worktree and target shares warm build artifacts cleanly).\n- Measure every UI change at 1×, 1.5× and 2×."
    } else if mode == "tall" {
        "## Style\n- Keep functions small and readable.\n- Build Rust crates with jato.\n- Measure every UI change at 1×, 1.5× and 2×.\n- Keep colors from the theme palette.\n- Keep spacing from kit metrics.\n- Never block the UI thread on I/O.\n- Stream long results with progress.\n- Test headless with tsession.\n- Keep scratch files under /var/tmp.\n- Prefer native Rust crates over shelling out.\n- Document new kit widgets in HACKING.md.\n- Keep window state across in-place reloads.\n- Run ui-lint on every layout change."
    } else {
        "## Style\n- Keep functions small and readable.\n- Build Rust crates with jato.\n- Measure every UI change at 1×, 1.5× and 2×."
    };
    let items = vec![
        DiffItem {
            id: ".claude/settings.json::permissions.allow".into(),
            file: ".claude/settings.json".into(),
            item: "permissions.allow".into(),
            kind: DiffKind::Conflict,
            can_merge: true,
            here: "[\n  \"Bash(jato build *)\",\n  \"Bash(jato test *)\",\n  \"Bash(git status)\"\n]".into(),
            there: Some("[\n  \"Bash(jato test *)\",\n  \"Bash(podman build *)\",\n  \"Bash(systemctl --user *)\"\n]".into()),
            merged_preview: Some("[\n  \"Bash(jato build *)\",\n  \"Bash(jato test *)\",\n  \"Bash(git status)\",\n  \"Bash(podman build *)\",\n  \"Bash(systemctl --user *)\"\n]".into()),
            remembered: Some(Choice::Merge),
            resolved_same: false,
        },
        DiffItem {
            id: ".claude/settings.json::model".into(),
            file: ".claude/settings.json".into(),
            item: "model".into(),
            kind: DiffKind::Conflict,
            can_merge: false,
            here: "\"opus-5.5\"".into(),
            there: Some("\"sonnet-4.5\"".into()),
            merged_preview: None,
            remembered: None,
            resolved_same: false,
        },
        DiffItem {
            id: ".codex/config.toml::mcp_servers.docs".into(),
            file: ".codex/config.toml".into(),
            item: "mcp_servers.docs".into(),
            kind: DiffKind::Conflict,
            can_merge: true,
            here: "[docs]\nurl = \"https://docs.example/v2/mcp\"\ntimeout_secs = 30".into(),
            there: Some("[docs]\nurl = \"https://docs.example/v1/mcp\"\nregion = \"eu-west\"".into()),
            merged_preview: Some("[docs]\nurl = \"https://docs.example/v2/mcp\"\nregion = \"eu-west\"\ntimeout_secs = 30".into()),
            remembered: None,
            resolved_same: false,
        },
        DiffItem {
            id: ".claude/CLAUDE.md::## Style".into(),
            file: ".claude/CLAUDE.md".into(),
            item: "## Style".into(),
            kind: DiffKind::Conflict,
            can_merge: true,
            here: long_style_here.into(),
            there: Some("## Style\n- Keep functions small and readable.\n- This server runs Fedora 44 with Podman.".into()),
            merged_preview: Some(format!("{long_style_here}\n- This server runs Fedora 44 with Podman.")),
            remembered: None,
            resolved_same: false,
        },
        DiffItem {
            id: ".claude/commands/ship.md".into(),
            file: ".claude/commands/ship.md".into(),
            item: String::new(),
            kind: DiffKind::Conflict,
            can_merge: false,
            here: "Run tests, build release, push, and publish GitHub release.".into(),
            there: Some("Run staging deploy script and tag release.".into()),
            merged_preview: None,
            remembered: None,
            resolved_same: false,
        },
        DiffItem {
            id: ".claude/skills/jato/SKILL.md".into(),
            file: ".claude/skills/jato/SKILL.md".into(),
            item: String::new(),
            kind: DiffKind::New,
            can_merge: false,
            here: "---\nname: jato\ndescription: Use Jato for Rust Cargo builds.\n---".into(),
            there: None,
            merged_preview: None,
            remembered: None,
            resolved_same: false,
        },
        DiffItem {
            id: ".codex/config.toml::approval_policy".into(),
            file: ".codex/config.toml".into(),
            item: "approval_policy".into(),
            kind: DiffKind::New,
            can_merge: false,
            here: "approval_policy = \"on-request\"".into(),
            there: None,
            merged_preview: None,
            remembered: None,
            resolved_same: false,
        },
    ];
    let demo_files = vec![
        olo_rex_client::carry::File {
            path: ".claude/settings.json".into(),
            bytes: br#"{"model":"opus-5.5","permissions":{"allow":["Bash(jato build *)","Bash(jato test *)","Bash(git status)"]}}"#.to_vec(),
            runs: false,
            taken_back: None,
        },
        olo_rex_client::carry::File {
            path: ".codex/config.toml".into(),
            bytes: b"[mcp_servers.docs]\nurl = \"https://docs.example/v2/mcp\"\ntimeout_secs = 30\napproval_policy = \"on-request\"\n".to_vec(),
            runs: false,
            taken_back: None,
        },
        olo_rex_client::carry::File { path: ".claude/CLAUDE.md".into(), bytes: format!("{long_style_here}\n").into_bytes(), runs: false, taken_back: None },
        olo_rex_client::carry::File { path: ".claude/commands/ship.md".into(), bytes: b"Run tests, build release, push, and publish GitHub release.\n".to_vec(), runs: false, taken_back: None },
        olo_rex_client::carry::File {
            path: ".claude/skills/jato/SKILL.md".into(),
            bytes: b"---\nname: jato\ndescription: Use Jato for Rust Cargo builds.\n---\n".to_vec(),
            runs: false,
            taken_back: None,
        },
    ];
    let mut remote_files = HashMap::new();
    remote_files.insert(".claude/settings.json".into(), br#"{"model":"sonnet-4.5","permissions":{"allow":["Bash(jato test *)","Bash(podman build *)","Bash(systemctl --user *)"]}}"#.to_vec());
    remote_files.insert(".codex/config.toml".into(), b"[mcp_servers.docs]\nurl = \"https://docs.example/v1/mcp\"\nregion = \"eu-west\"\n".to_vec());
    remote_files.insert(".claude/CLAUDE.md".into(), b"## Style\n- Keep functions small and readable.\n- This server runs Fedora 44 with Podman.\n".to_vec());
    remote_files.insert(".claude/commands/ship.md".into(), b"Run staging deploy script and tag release.\n".to_vec());
    let plan = Plan { home: "/home/you".into(), files: demo_files, remote_files, items, same_files: 6, remembered_total: remembered.len(), ..Plan::default() };
    let demo_selection = match mode.as_str() {
        "multi" => Some(vec![0, 1, 2]),
        "new-multi" => Some(vec![4, 5]),
        "json" => Some(vec![2]),
        "remembered" => Some(vec![1]),
        _ => None,
    };
    let mut shown = Shown { key: key.to_owned(), name: name.to_owned(), tab: usize::from(mode == "remembered"), preview_result: mode == "merged", demo_selection, ..Shown::default() };
    install_plan(&mut shown, plan, remembered);
    // 2 of 4 conflicts explicitly decided (model = Copy, ship.md = Skip + Remember);
    // the other 2 conflicts (## Style, mcp_servers.docs) show suggested Merge in secondary ink.
    shown.choices.insert(".claude/settings.json::model".into(), Choice::TakeDesktop);
    shown.decided.insert(".claude/settings.json::model".into());
    shown.choices.insert(".claude/commands/ship.md".into(), Choice::KeepHost);
    shown.decided.insert(".claude/commands/ship.md".into());
    shown.remember.insert(".claude/commands/ship.md".into(), true);
    shown
}
