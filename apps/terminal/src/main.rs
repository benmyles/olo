//! Terminal: a shell in a window, BeOS style (one process, any number of
//! windows), on alacritty's terminal emulator. Drawn by GPUI (ui.rs, view.rs),
//! with Olo's own light and dark colour schemes and Catppuccin's
//! (colors.rs), one for each appearance. A window can hold
//! several shells as tabs, and be a named, coloured group of them that
//! can be saved and opened again (groups.rs). Tabs running a coding agent
//! show what it's doing, and say when it needs you (agent.rs).
//!
//! The shells run in olo-termd (termd.rs), so they outlive Terminal:
//! quit it, or have it crash or upgrade, and the next Terminal opens a
//! window on each shell as it was, scrollback and all.

mod agent;
mod agent_context;
mod agent_title;
mod client;
mod colors;
mod daemon;
mod find;
mod git;
mod graphics;
mod groups;
mod io;
mod links;
mod repos;
mod session;
mod sudo;
mod sudo_wire;
mod ui;
mod view;
#[allow(dead_code)]
mod wire;

use std::{collections::HashMap, path::PathBuf, rc::Rc, sync::Arc, time::Instant};

use alacritty_terminal::grid::Dimensions;
use olo_kit_base::{
    instance::{self, Launch},
    metrics as m,
    notify::{Note, NoteEvent, NoteId, NoteKind, Notifier},
};
use olo_theme::{Palette, ThemeMode, looks};
use olo_ui::{
    Menu,
    gpui::{App, AppContext as _, Entity},
    menu,
    widgets::{self, TextField},
};

use agent::{Agent, State};
use colors::Scheme;
use groups::Group;
use session::{Message, Session, Size, Start, TermEvent};
use ui::{Cx, WinOps as _, WindowId, WindowOptions};
use view::{Metrics, Mouse};

pub(crate) const APP_ID: &str = "olo.terminal";
/// BeOS Terminal opened at 80×25; wider is more useful on today's screens.
const DEFAULT_SIZE: (usize, usize) = (120, 25);
pub const TEXT_SIZES: [f32; 9] = [9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 16.0, 18.0, 22.0];
const DEFAULT_TEXT_SIZE: f32 = 12.0;
/// The tab strip, shown once a window has more than one tab.
const STRIP: f32 = widgets::STRIP_HEIGHT;
/// A group's menu (its chip's, in the strip, and its row's divider in
/// Tile All Tabs).
pub(crate) fn group_menu(g: usize, t: &TermWin) -> Menu {
    Menu::new("")
        .item("Rename Group…", format!("group-rename:{g}"))
        .item("New Tab in Group", format!("group-new-tab:{g}"))
        .separator()
        .item("Save Group", format!("group-save:{g}"))
        .item("Move Group to New Window", format!("group-window:{g}"))
        .enabled(t.tabs.iter().any(|x| x.group != Some(g)))
        .separator()
        .item("Ungroup", format!("group-ungroup:{g}"))
        .item("Close Group", format!("group-close:{g}"))
        .separator()
        .item("Tile All Tabs", "tile-tabs")
        .checked(t.tiled)
}

/// Show `t`'s title on its window, and its folder, if it changed.
fn retitle(cx: &mut Cx, win: WindowId, t: &mut TermWin) {
    if let Some(title) = t.retitle() {
        cx.set_title(win, title);
        // Shells retitle at each prompt, so after a `cd` too.
        cx.set_path(win, tab_folder(t.tab()));
    }
}

/// The folder a tab works in: its foreground program's, else its shell's.
fn tab_folder(tab: &Tab) -> Option<PathBuf> {
    let shared = &tab.session.shared;
    shared.foreground().or(shared.shell()).and_then(|pid| std::fs::read_link(format!("/proc/{pid}/cwd")).ok())
}

/// The tab's folder (what its program, else its shell, is in, as its
/// title shows) in Tracker.
fn open_in_tracker(tab: &Tab) {
    if let Some(folder) = tab_folder(tab)
        && let Err(e) = olo_kit_base::tracker::open(&folder)
    {
        tracing::warn!("couldn't open {} in Tracker: {e}", folder.display());
    }
}

/// Which of the two schemes (`Settings::light_colors`, `dark_colors`)
/// Terminal draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
enum SchemeChoice {
    /// Follow the desktop's light/dark appearance.
    #[default]
    Auto,
    /// Always the light-mode one.
    Light,
    /// Always the dark-mode one.
    Dark,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Settings {
    text_size: f32,
    scheme: SchemeChoice,
    cols: usize,
    rows: usize,
    /// The colours in light mode, and in dark mode: a terminal's id
    /// (`looks::terminal`), or none for the look's own. Choosing another
    /// look in Preferences puts both back to its own.
    #[serde(default)]
    light_colors: Option<String>,
    #[serde(default)]
    dark_colors: Option<String>,
    /// The look (theme, colour set) they last followed.
    #[serde(default)]
    look: Option<(String, String)>,
    /// Tabs down the left side rather than along the top, and how wide
    /// that strip is.
    #[serde(default)]
    side_tabs: bool,
    #[serde(default = "side_width")]
    side_width: f32,
    /// sudo typed at a shell asks for its password in a window too (a
    /// command with no terminal, as an agent's, always does).
    #[serde(default = "yes")]
    sudo_window: bool,
}

fn yes() -> bool {
    true
}

fn side_width() -> f32 {
    widgets::STRIP_SIDE_WIDTH
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            text_size: DEFAULT_TEXT_SIZE,
            scheme: SchemeChoice::Auto,
            cols: DEFAULT_SIZE.0,
            rows: DEFAULT_SIZE.1,
            light_colors: None,
            dark_colors: None,
            look: None,
            side_tabs: false,
            side_width: side_width(),
            sudo_window: true,
        }
    }
}

fn settings_path() -> PathBuf {
    olo_xdg::config_dir().join("terminal.json")
}

impl Settings {
    fn load() -> Settings {
        std::fs::read_to_string(settings_path()).ok().map(|t| Settings::parse(&t)).unwrap_or_default()
    }

    fn parse(text: &str) -> Settings {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { return Settings::default() };
        // From before there was a choice for each appearance, Light and
        // Dark were Olo Light and Olo Dark (now BeOS's), always: still so.
        let before = value.get("light_colors").is_none() && value.get("dark_colors").is_none();
        let mut s: Settings = serde_json::from_value(value).unwrap_or_default();
        if before {
            match s.scheme {
                SchemeChoice::Light => s.light_colors = Some("beos-light".into()),
                SchemeChoice::Dark => s.dark_colors = Some("beos-dark".into()),
                SchemeChoice::Auto => {}
            }
        }
        for c in [&mut s.light_colors, &mut s.dark_colors] {
            if let Some(id) = c.as_mut()
                && let Some(mode) = id.strip_prefix("olo-")
            {
                *id = format!("beos-{mode}");
            }
        }
        s
    }

    fn save(&self) {
        let path = settings_path();
        let text = serde_json::to_string_pretty(self).unwrap_or_default();
        // Off the UI thread: the disk can be slow.
        std::thread::spawn(move || {
            if let Some(d) = path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let tmp = path.with_extension("json.new");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        });
    }
}

/// One shell, in a tab of a window.
pub(crate) struct Tab {
    session: Session,
    mouse: Mouse,
    /// What drawing its screen keeps between frames.
    pane: Rc<view::Pane>,
    /// What its tab says: the program's own title, else what's running.
    title: String,
    /// A visual bell: when it rang.
    bell: Option<Instant>,
    /// The bell rang while another tab was showing.
    rang: bool,
    /// The coding agent running in it, and what it's doing.
    agent: Option<(Agent, State)>,
    /// What its agent says it's working on (agent_title.rs): its title,
    /// ahead of the program's own, until the agent leaves.
    agent_title: Option<String>,
    /// A title back from before Terminal restarted: kept if its agent is
    /// seen by then, else let go.
    agent_title_until: Option<Instant>,
    /// Its agent stopped (done, or asking) while no one was looking.
    attention: bool,
    /// The git repository its folder is in (repos.rs).
    repo: Option<git::Repo>,
    /// The group it's in (into its window's `groups`).
    group: Option<usize>,
    /// The person took it out of a group: it doesn't join its
    /// repository's group on its own again (repos.rs).
    own_grouping: bool,
}

impl Tab {
    fn new(session: Session) -> Tab {
        Tab {
            session,
            mouse: Mouse::default(),
            pane: Rc::default(),
            title: "Terminal".into(),
            bell: None,
            rang: false,
            agent: None,
            agent_title: None,
            agent_title_until: None,
            attention: false,
            repo: None,
            group: None,
            own_grouping: false,
        }
    }

    /// What it's doing, for its tab's name to screen readers and agents.
    fn status(&self) -> Option<String> {
        match self.agent {
            Some((agent, state)) => Some(format!("{}: {}", agent.name(), state.describe())),
            None => self.rang.then(|| "bell rang".into()),
        }
    }

    fn current_title(&self) -> String {
        self.agent_title
            .clone()
            .or_else(|| self.session.title.clone().filter(|s| !s.trim().is_empty()))
            .or_else(|| self.session.foreground())
            .unwrap_or_else(|| "Terminal".into())
    }

    /// Tell the program whether it has the keyboard, if it asked to know.
    fn report_focus(&self, focused: bool) {
        let mut term = self.session.term.lock();
        term.is_focused = focused;
        if term.mode().contains(alacritty_terminal::term::TermMode::FOCUS_IN_OUT) {
            drop(term);
            self.session.write(if focused { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() });
        }
    }
}

pub(crate) struct TermWin {
    tabs: Vec<Tab>,
    active: usize,
    /// Text an input method is composing, at the cursor.
    preedit: Option<String>,
    /// A redraw is coming for an agent's spinner.
    spinning: bool,
    /// Tile All Tabs' divider being dragged (`ui::DividerDrag`).
    dragging: Option<ui::DividerDrag>,
    /// Size to snap the window to once real font metrics are known.
    snap_to: Option<(usize, usize)>,
    shown_title: String,
    focused: bool,
    /// Its tabs' groups (`Tab::group`): each one's tabs side by side,
    /// in the order they first come (`tidy`).
    groups: Vec<Group>,
    /// Tile All Tabs: every tab at once, tiled in the window, a row for
    /// each group (`tile_rows`).
    tiled: bool,
    /// The groups chosen in Tile All Tabs (by name: groups renumber),
    /// the only ones tiled; none (or all): every tab.
    tile_groups: Vec<String>,
    /// Each row's share of the height in Tile All Tabs, as the person
    /// dragged it, by its group's name ("" for tabs in none); 1 if not.
    tile_heights: std::collections::BTreeMap<String, f32>,
    /// Each row's panes' shares of its width, as dragged, by the row's
    /// key; even if not (or if the row has a pane more or fewer since).
    tile_widths: std::collections::BTreeMap<String, Vec<f32>>,
}

impl TermWin {
    /// A window of `tabs` (each saying its group in `groups`).
    fn new(tabs: Vec<Tab>, active: usize, snap_to: Option<(usize, usize)>, groups: Vec<Group>) -> TermWin {
        let active = active.min(tabs.len().saturating_sub(1));
        let mut t = TermWin { active, tabs, preedit: None, spinning: false, dragging: None, snap_to, shown_title: String::new(), focused: true, groups, tiled: false, tile_groups: Vec::new(), tile_heights: Default::default(), tile_widths: Default::default() };
        t.tidy();
        t
    }

    /// The group of the tab showing.
    fn group(&self) -> Option<&Group> {
        self.tab().group.and_then(|g| self.groups.get(g))
    }

    /// Each group's tabs side by side (where its first is), groups no tab
    /// is in gone, the rest in the order they come; the same tab showing.
    fn tidy(&mut self) {
        let Some(shown) = self.tabs.get(self.active).map(|t| t.session.id) else { return };
        let mut rest: Vec<Option<Tab>> = std::mem::take(&mut self.tabs).into_iter().map(Some).collect();
        let mut order = Vec::with_capacity(rest.len());
        for i in 0..rest.len() {
            let Some(tab) = rest[i].take() else { continue };
            let g = tab.group;
            order.push(tab);
            if g.is_some() {
                for later in rest.iter_mut().skip(i + 1) {
                    if later.as_ref().is_some_and(|t| t.group == g) {
                        order.extend(later.take());
                    }
                }
            }
        }
        let mut new_index: Vec<Option<usize>> = vec![None; self.groups.len()];
        let mut groups = Vec::new();
        for t in order.iter_mut() {
            t.group = t.group.filter(|&g| g < new_index.len()).map(|g| {
                *new_index[g].get_or_insert_with(|| {
                    groups.push(self.groups[g].clone());
                    groups.len() - 1
                })
            });
        }
        self.groups = groups;
        self.tabs = order;
        self.active = self.position(shown).unwrap_or(0);
    }

    /// Group `g`'s tabs (their places).
    fn members(&self, g: usize) -> Vec<usize> {
        (0..self.tabs.len()).filter(|&i| self.tabs[i].group == Some(g)).collect()
    }

    /// Tile All Tabs' rows: each group's tabs (in the strip's order), and
    /// the tabs in none together where the first of them is; only the
    /// chosen groups' if some are (not all).
    fn tile_rows(&self) -> Vec<Vec<usize>> {
        let mut rows: Vec<(Option<usize>, Vec<usize>)> = Vec::new();
        for (i, tab) in self.tabs.iter().enumerate() {
            match rows.iter_mut().find(|(g, _)| *g == tab.group) {
                Some((_, row)) => row.push(i),
                None => rows.push((tab.group, vec![i])),
            }
        }
        let chosen: Vec<usize> = (0..self.groups.len()).filter(|&g| self.tile_groups.contains(&self.groups[g].name)).collect();
        if !chosen.is_empty() && chosen.len() < self.groups.len() {
            rows.retain(|(g, _)| g.is_some_and(|g| chosen.contains(&g)));
        }
        rows.into_iter().map(|(_, row)| row).collect()
    }

    /// A Tile All Tabs row's name for its share of the height: its
    /// group's, or "" for the tabs in none.
    fn row_key(&self, row: &[usize]) -> String {
        row.first().and_then(|&i| self.tabs.get(i)).and_then(|t| t.group).and_then(|g| self.groups.get(g)).map(|g| g.name.clone()).unwrap_or_default()
    }

    /// Whether the rows' heights or the panes' widths were dragged from
    /// even.
    fn sizes_changed(&self) -> bool {
        self.tile_heights.values().any(|w| (w - 1.0).abs() > 0.01) || self.tile_widths.values().flatten().any(|w| (w - 1.0).abs() > 0.01)
    }

    /// Choose group `g` in Tile All Tabs, or not; all chosen is none.
    /// The tab typed into stays one that shows.
    fn toggle_tile_group(&mut self, g: usize) {
        let Some(name) = self.groups.get(g).map(|x| x.name.clone()) else { return };
        if let Some(i) = self.tile_groups.iter().position(|n| *n == name) {
            self.tile_groups.remove(i);
        } else {
            self.tile_groups.push(name);
        }
        self.tile_groups.retain(|n| self.groups.iter().any(|x| x.name == *n));
        if self.tile_groups.len() >= self.groups.len() {
            self.tile_groups.clear();
        }
        let shown: Vec<usize> = self.tile_rows().concat();
        if !shown.contains(&self.active)
            && let Some(&first) = shown.first()
        {
            self.select(first);
        }
    }

    /// Each tab's title, and the window's (the one showing, after its
    /// group's name) if that changed.
    fn retitle(&mut self) -> Option<String> {
        for tab in self.tabs.iter_mut() {
            tab.title = tab.current_title();
        }
        let title = match self.group() {
            Some(g) => format!("{} — {}", g.name, self.tab().title),
            None => self.tab().title.clone(),
        };
        (title != self.shown_title).then(|| {
            self.shown_title = title.clone();
            title
        })
    }

    fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    fn grid(&self) -> (usize, usize) {
        let s = self.tab().session.size;
        (s.cols, s.rows)
    }

    fn position(&self, shell: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.session.id == shell)
    }

    fn select(&mut self, i: usize) {
        if i >= self.tabs.len() || i == self.active {
            return;
        }
        if self.focused {
            self.tab().report_focus(false);
        }
        self.active = i;
        self.tabs[i].rang = false;
        self.tabs[i].attention = false;
        if self.focused {
            self.tabs[i].report_focus(true);
        }
    }

    /// Take tab `i` out; the one after it (or else before) shows.
    fn take(&mut self, i: usize) -> Tab {
        let was_active = i == self.active;
        let tab = self.tabs.remove(i);
        if self.active > i {
            self.active -= 1;
        }
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
        if was_active && let Some(t) = self.tabs.get_mut(self.active) {
            t.rang = false;
            if self.focused {
                t.report_focus(true);
            }
        }
        // The strip can go with the second tab; the window stays as it is.
        tab
    }
}

pub(crate) enum Win {
    Term(Box<TermWin>),
    /// "Close this tab (or window)?" for one still running something.
    ConfirmClose { target: WindowId, shell: Option<u64>, message: String },
    /// The Find window for a terminal window.
    Find { target: WindowId, field: Entity<TextField> },
    /// Terminal ▸ Terminal Daemon…
    Daemon(Box<daemon::DaemonWin>),
    /// "Start olo-termd over?" for the Terminal Daemon window.
    ConfirmStartOver { target: WindowId },
    /// Name (or rename) a group of a window's (`group`), or make one of
    /// its tab `tab`, and choose its colour.
    NameGroup { target: WindowId, group: Option<usize>, tab: u64, name: Entity<TextField>, color: usize },
    /// sudo asks for its password (sudo.rs).
    Sudo(Box<sudo::Request>),
    /// New Worktree… or Rename Worktree… (repos.rs).
    Worktree(Box<repos::Dialog>),
    /// "Remove the worktree?"
    ConfirmRemove { main: PathBuf, path: PathBuf, message: String },
    /// What went wrong, with OK.
    Failed { message: String },
}

pub(crate) struct Terminal {
    link: client::Link,
    windows: HashMap<WindowId, Win>,
    settings: Settings,
    metrics: Option<Metrics>,
    /// The desktop's palette now (its look, its mode).
    look: Arc<Palette>,
    search: find::Search,
    /// olo-termd was started over: when its shells' windows have all
    /// closed, open a new one rather than quitting.
    respawn: bool,
    /// The terminal window last in front, for new tabs from outside.
    front: Option<WindowId>,
    /// Groups saved to open again (groups.rs).
    saved_groups: Vec<groups::Saved>,
    agents: agent::Watcher,
    /// Each tab's repository, its worktrees and pull requests (git.rs).
    git: git::Watcher,
    worktrees: HashMap<PathBuf, Vec<git::Worktree>>,
    branches: HashMap<PathBuf, Vec<String>>,
    prs: HashMap<git::PrKey, git::Pr>,
    /// Preferences ▸ Terminal: where new worktrees go.
    worktrees_at: String,
    notifier: Notifier,
    /// The notification each shell's agent last put up.
    notes: HashMap<u64, NoteId>,
    /// The windows as last kept on disk (`remember`).
    remembered: Option<String>,
    /// A `remember` is due soon (changes come in bursts).
    remembering: bool,
    /// The titles are to be looked at again soon (after typing: a program
    /// may have started without writing anything).
    retitling: bool,
    /// Preferences ▸ Terminal: coding agents told about their tab's group
    /// (agent_context.rs) and keeping its title (agent_title.rs; None:
    /// not heard yet), and what was last kept for their hook and tool.
    agent_context: Option<(bool, bool)>,
    agent_snapshot: Option<agent_context::Snapshot>,
    /// sudo requests approved without asking, by connection, with the
    /// window they came from (a window asks if the password doesn't do).
    sudo_approved: HashMap<u64, (Box<sudo::Request>, Option<WindowId>)>,
}


/// Preferences ▸ Terminal's coding agent settings, as they change: told
/// about their tab's group (agent_context.rs), and keeping their tab's
/// title current (agent_title.rs).
struct AgentContext {
    groups: bool,
    titles: bool,
}

/// How soon a tab's agent must be seen again after a restart to keep
/// its title.
const AGENT_SEEN_WITHIN: std::time::Duration = std::time::Duration::from_secs(5);

/// How soon after a change the windows are kept (changes come in
/// bursts), besides when Terminal quits.
const REMEMBER_AFTER: std::time::Duration = std::time::Duration::from_secs(2);

/// How soon after typing the tabs' titles are looked at again: a program
/// started may write nothing (its name is the title).
const RETITLE_AFTER: std::time::Duration = std::time::Duration::from_millis(400);

/// Where Terminal keeps its windows for the next time it starts, beside
/// the shells olo-termd keeps (only for us: folders say what you work on).
fn layout_path() -> PathBuf {
    olo_xdg::state_dir().join("terminal").join("windows.json")
}

/// The windows as Terminal last kept them.
fn remembered() -> Option<Saved> {
    serde_json::from_str(&std::fs::read_to_string(layout_path()).ok()?).ok()
}

/// Room beside and above the grid: the menu bar, and the tab strip (down
/// the side or along the top), which shows with one tab too.
pub(crate) fn chrome(settings: &Settings) -> (u32, u32) {
    if settings.side_tabs {
        (settings.side_width as u32, menu::BAR_HEIGHT as u32)
    } else {
        (0, (menu::BAR_HEIGHT + STRIP) as u32)
    }
}

/// Estimated content size for `cols`×`rows` before fonts are loaded
/// (corrected after the first frame).
fn estimate(settings: &Settings, cols: usize, rows: usize) -> (u32, u32) {
    let s = settings.text_size;
    let cell_w = (s * 0.6 * 2.0).ceil() / 2.0;
    let cell_h = (s * 1.32 * 2.0).ceil() / 2.0;
    let (w, h) = chrome(settings);
    (
        (cols as f32 * cell_w + 2.0 * view::PAD.0).ceil() as u32 + w,
        (rows as f32 * cell_h + 2.0 * view::PAD.1).ceil() as u32 + h,
    )
}

/// What an upgraded Terminal gets from its previous self, and what the
/// next Terminal starts with (`layout_path`): which shells were in each
/// window, and where (olo-termd has the rest, or keeps it when the
/// session ends).
#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    windows: Vec<SavedWindow>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedWindow {
    /// The shell showing (all there was before tabs).
    shell: u64,
    /// Every tab's shell, in order.
    #[serde(default)]
    tabs: Vec<u64>,
    #[serde(default)]
    active: usize,
    size: (u32, u32),
    /// The window's group, before tabs had groups of their own: every
    /// tab in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    group: Option<Group>,
    #[serde(default)]
    tiled: bool,
    /// The groups chosen in Tile All Tabs (`TermWin::tile_groups`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tile_groups: Vec<String>,
    /// Its rows' shares of the height (`TermWin::tile_heights`).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    tile_heights: std::collections::BTreeMap<String, f32>,
    /// Its rows' panes' shares of the width (`TermWin::tile_widths`).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    tile_widths: std::collections::BTreeMap<String, Vec<f32>>,
    /// Each tab's folder, for a shell in its place if it has gone.
    #[serde(default)]
    cwds: Vec<Option<PathBuf>>,
    /// Likewise, from before: every tab taken out of its group.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    own_grouping: bool,
    /// Its groups, and each tab's (into them), and which tabs the person
    /// took out of a group.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    groups: Vec<Group>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tab_groups: Vec<Option<usize>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    own: Vec<bool>,
    /// Each tab's coding agent's title (`Tab::agent_title`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    agent_titles: Vec<Option<String>>,
}

impl SavedWindow {
    /// Its groups (the window's one, from before).
    fn all_groups(&self) -> Vec<Group> {
        if self.groups.is_empty() { self.group.clone().into_iter().collect() } else { self.groups.clone() }
    }

    /// Tab `i`'s group, and whether the person took it out of one.
    fn tab_group(&self, i: usize) -> (Option<usize>, bool) {
        let group = if self.groups.is_empty() { self.group.as_ref().map(|_| 0) } else { self.tab_groups.get(i).copied().flatten() };
        (group, self.own.get(i).copied().unwrap_or(self.own_grouping))
    }
}

/// A new window's groups, and each tab's (into them) and whether the
/// person took it out of one (`Tab::own_grouping`).
type Grouping = (Vec<Group>, Vec<(Option<usize>, bool)>);

/// What a Terminal from before olo-termd handed on: the shells
/// themselves, which go to olo-termd now.
#[derive(serde::Deserialize)]
struct Legacy {
    windows: Vec<LegacyWindow>,
}

#[derive(serde::Deserialize)]
struct LegacyWindow {
    fd: i32,
    pid: i32,
    size: (u32, u32),
    cols: usize,
    rows: usize,
    title: Option<String>,
    contents: String,
}

impl Terminal {
    fn scheme(&self) -> Scheme {
        let mode = match self.settings.scheme {
            SchemeChoice::Auto => self.look.mode,
            SchemeChoice::Light => ThemeMode::Light,
            SchemeChoice::Dark => ThemeMode::Dark,
        };
        Scheme::from(&self.colors_for(mode))
    }

    /// The colours for `mode`: the ones chosen, else the look's own.
    fn colors_for(&self, mode: ThemeMode) -> olo_theme::Terminal {
        let chosen = match mode {
            ThemeMode::Light => &self.settings.light_colors,
            ThemeMode::Dark => &self.settings.dark_colors,
        };
        chosen.as_deref().and_then(looks::terminal).unwrap_or_else(|| looks::palette(&self.look.theme, &self.look.colors, mode).terminal)
    }

    /// Follow the desktop's look: when it is a new one (another theme or
    /// colour set; not just light or dark), its colours replace any
    /// chosen here.
    fn follow(&mut self, palette: &Arc<Palette>) {
        let look = (palette.theme.clone(), palette.colors.clone());
        if self.settings.look.as_ref() != Some(&look) {
            // The first look seen (as on the first run of this version)
            // keeps what was chosen before.
            if self.settings.look.is_some() {
                self.settings.light_colors = None;
                self.settings.dark_colors = None;
            }
            self.settings.look = Some(look);
            self.settings.save();
        }
        self.look = palette.clone();
    }

    fn open(&mut self, cx: &mut Cx, cwd: Option<PathBuf>, command: Option<Vec<String>>) {
        let (cols, rows) = (self.settings.cols, self.settings.rows);
        let options = self.window_options(cols, rows);
        self.add(cx, options, Size { cols, rows }, Some((cols, rows)), vec![Start::Spawn { command, cwd }], 0, Default::default());
    }

    /// A window on shells olo-termd already has (sized as it was, if
    /// we know), one tab each.
    /// A window of `starts`, at the grid of `like` (a shell olo-termd
    /// has; else the usual one).
    fn reopen(
        &mut self,
        cx: &mut Cx,
        starts: Vec<Start>,
        like: Option<&wire::ShellInfo>,
        size: Option<(u32, u32)>,
        active: usize,
        groups: Grouping,
    ) -> WindowId {
        let active = active.min(starts.len().saturating_sub(1));
        let (cols, rows) = like.map_or((self.settings.cols, self.settings.rows), |s| (s.grid.cols.max(1) as usize, s.grid.rows.max(1) as usize));
        let mut options = self.window_options(cols, rows);
        if let Some(size) = size {
            options.size = size;
        }
        if let Some(t) = like.and_then(|s| s.title.as_ref()) {
            options.title = t.clone();
        }
        let snap = size.is_none().then_some((cols, rows));
        self.add(cx, options, Size { cols, rows }, snap, starts, active, groups)
    }

    fn start(&self, cx: &Cx, size: Size, start: Start) -> Tab {
        let cell = self.metrics.map(|m| m.device_cell()).unwrap_or((15, 36));
        let session = Session::start(cx.proxy(), &self.link, size, cell, self.scheme(), start);
        self.agents.watch(session.id, &session.term, &session.shared);
        self.git.watch(session.id, &session.shared);
        Tab::new(session)
    }

    #[allow(clippy::too_many_arguments)]
    fn add(&mut self, cx: &mut Cx, options: WindowOptions, size: Size, snap_to: Option<(usize, usize)>, starts: Vec<Start>, active: usize, (groups, of): Grouping) -> WindowId {
        let id = cx.open_window(options);
        let tabs = starts
            .into_iter()
            .enumerate()
            .map(|(i, s)| {
                let mut tab = self.start(cx, size, s);
                (tab.group, tab.own_grouping) = of.get(i).copied().unwrap_or_default();
                tab
            })
            .collect();
        self.windows.insert(id, Win::Term(Box::new(TermWin::new(tabs, active, snap_to, groups))));
        id
    }

    /// A new tab in `win`, after the one showing (in its group, if it's
    /// in one, as a browser's), and show it.
    fn add_tab(&mut self, cx: &mut Cx, win: WindowId, start: Start) {
        self.add_tab_at(cx, win, start, None);
    }

    /// `add_tab`, or with `into`, at the end of that group, in it.
    fn add_tab_at(&mut self, cx: &mut Cx, win: WindowId, start: Start, into: Option<usize>) {
        let Some(Win::Term(t)) = self.windows.get(&win) else { return };
        let (cols, rows) = t.grid();
        let mut tab = self.start(cx, Size { cols, rows }, start);
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let (at, group) = match into.filter(|&g| g < t.groups.len()) {
            Some(g) => (t.members(g).last().map_or(t.tabs.len(), |&i| i + 1), Some(g)),
            None => (t.active + 1, t.tab().group),
        };
        tab.group = group;
        t.tabs.insert(at, tab);
        t.select(at);
        cx.redraw(win);
    }

    fn window_options(&self, cols: usize, rows: usize) -> WindowOptions {
        let (w, h) = match self.metrics {
            Some(m) => {
                let (w, h) = m.content_size(cols, rows);
                let (cw, ch) = chrome(&self.settings);
                (w as u32 + cw, h as u32 + ch)
            }
            None => estimate(&self.settings, cols, rows),
        };
        WindowOptions::new("Terminal", w, h).min_size(200, 120)
    }

    fn handle_launch(&mut self, cx: &mut Cx, launch: Launch) {
        // A coding agent's tab title (agent_title.rs), not a window.
        if launch.args.first().map(String::as_str) == Some(agent_title::SET_FLAG) {
            if let Some(pid) = launch.args.get(1).and_then(|p| p.parse().ok()) {
                self.set_agent_title(cx, pid, launch.args.get(2).map_or("", String::as_str));
            }
            return;
        }
        let mut cwd = launch.cwd;
        let mut command = None;
        let mut tab = false;
        let mut args = launch.args.into_iter();
        while let Some(a) = args.next() {
            match a.as_str() {
                "-w" | "--working-directory" => cwd = args.next().map(PathBuf::from),
                "--tab" => tab = true,
                "-e" | "--" => {
                    command = Some(args.by_ref().collect::<Vec<_>>());
                }
                _ => {}
            }
        }
        let command = command.filter(|c: &Vec<String>| !c.is_empty());
        // `--tab`: in the window last in front on this workspace, if
        // there is one (else a new window here).
        let here = |w: &WindowId| matches!(self.windows.get(w), Some(Win::Term(_))) && cx.on_this_workspace(*w);
        // A folder in a repository: at the end of its group, if one is here.
        let real = cwd.as_ref().and_then(|d| d.canonicalize().ok());
        let in_repo = |main: &PathBuf| {
            real.as_ref().is_some_and(|d| d.starts_with(main) || self.worktrees.get(main).is_some_and(|l| l.iter().any(|w| d.starts_with(&w.path))))
        };
        let repo_group = self.windows.iter().filter(|(w, _)| here(w)).find_map(|(&id, w)| match w {
            Win::Term(t) => t.groups.iter().position(|g| g.repo.as_ref().is_some_and(in_repo)).map(|g| (id, g)),
            _ => None,
        });
        let front = repo_group.map(|(id, _)| id).or(self.front.filter(here)).or_else(|| {
            let mut terms: Vec<WindowId> = self.windows.keys().copied().filter(here).collect();
            terms.sort();
            terms.last().copied()
        });
        match front.filter(|_| tab) {
            Some(win) => {
                self.add_tab_at(cx, win, Start::Spawn { command, cwd }, repo_group.map(|(_, g)| g));
                cx.activate(win);
            }
            None => self.open(cx, cwd, command),
        }
    }

    /// Open windows on the shells left in olo-termd: those this
    /// Terminal had before it upgraded (`saved`, with their tabs as they
    /// were), and any no other Terminal is showing. True if there were any.
    /// Open windows on olo-termd's `shells` as `saved` had them (the rest
    /// a window each). `restore`: a shell that has gone (the session it
    /// ran in ended) gets a new one in its place, in its folder, with what
    /// it showed (olo-termd kept it).
    fn reattach(&mut self, cx: &mut Cx, shells: Vec<wire::ShellInfo>, saved: Option<&Saved>, restore: bool) -> bool {
        let ours = std::process::id() as i32;
        // SAFETY: signal 0 only checks the process exists.
        let free = |s: &wire::ShellInfo| s.client.is_none_or(|p| p == ours || unsafe { libc::kill(p, 0) } != 0);
        let free: Vec<&wire::ShellInfo> = shells.iter().filter(|s| free(s)).collect();
        let mut used = std::collections::HashSet::new();
        let mut any = false;
        for w in saved.map(|s| s.windows.as_slice()).unwrap_or_default() {
            let ids = if w.tabs.is_empty() { vec![w.shell] } else { w.tabs.clone() };
            let shown = ids.get(w.active).copied();
            let (mut starts, mut active, mut grid) = (Vec::new(), 0, None);
            let mut grouping = Vec::new();
            for (i, &id) in ids.iter().enumerate() {
                let start = match free.iter().find(|s| s.id == id && !used.contains(&id)) {
                    Some(s) => {
                        used.insert(id);
                        if grid.is_none() || Some(id) == shown {
                            grid = Some(*s);
                        }
                        Start::Attach(id)
                    }
                    None if restore && !shells.iter().any(|s| s.id == id) => {
                        Start::Restore { cwd: w.cwds.get(i).cloned().flatten(), from: id }
                    }
                    None => continue,
                };
                if Some(id) == shown {
                    active = starts.len();
                }
                starts.push(start);
                grouping.push(w.tab_group(i));
            }
            if starts.is_empty() {
                continue;
            }
            any = true;
            let id = self.reopen(cx, starts, grid, Some(w.size), active, (w.all_groups(), grouping));
            if let Some(Win::Term(t)) = self.windows.get_mut(&id) {
                t.tiled = w.tiled;
                t.tile_groups = w.tile_groups.clone();
                t.tile_heights = w.tile_heights.clone();
                t.tile_widths = w.tile_widths.clone();
                // Agents' titles, back with their shells (each let go if
                // its agent isn't seen there soon: `expire_agent_titles`).
                for tab in t.tabs.iter_mut() {
                    let i = ids.iter().position(|&s| s == tab.session.id);
                    tab.agent_title = i.and_then(|i| w.agent_titles.get(i).cloned().flatten());
                    tab.agent_title_until = tab.agent_title.as_ref().map(|_| Instant::now() + AGENT_SEEN_WITHIN);
                }
                retitle(cx, id, t);
            }
        }
        let rest: Vec<&wire::ShellInfo> = free.iter().filter(|s| !used.contains(&s.id)).copied().collect();
        for shell in &rest {
            self.reopen(cx, vec![Start::Attach(shell.id)], Some(shell), None, 0, Default::default());
        }
        any || !rest.is_empty()
    }

    /// Hand the shells a Terminal from before olo-termd left us over
    /// to it, each in its window as before.
    fn adopt(&mut self, cx: &mut Cx, legacy: Legacy) -> bool {
        let mut any = false;
        for w in legacy.windows {
            // SAFETY: our previous self passed this descriptor to us alone.
            if unsafe { libc::fcntl(w.fd, libc::F_GETFD) } < 0 {
                continue;
            }
            // SAFETY: as above; it's ours now.
            let pty = unsafe { std::os::fd::FromRawFd::from_raw_fd(w.fd) };
            let mut options = self.window_options(w.cols, w.rows);
            options.size = w.size;
            let size = Size { cols: w.cols.max(1), rows: w.rows.max(1) };
            self.add(cx, options, size, None, vec![Start::Adopt { pty, pid: w.pid, title: w.title, replay: w.contents }], 0, Default::default());
            any = true;
        }
        any
    }

    fn set_text_size(&mut self, cx: &mut Cx, size: f32) {
        self.settings.text_size = size;
        self.settings.save();
        self.metrics = None;
        // Keep each window's grid size: resize the windows to fit. Not
        // those away (another workspace, hidden): they draw nothing till
        // they are back, and would jump then; they keep their size, and
        // their grid fits it.
        for (id, w) in &mut self.windows {
            if let Win::Term(t) = w {
                t.snap_to = cx.is_shown(*id).then(|| t.grid());
                cx.redraw(*id);
            }
        }
    }

    fn set_grid(&mut self, cx: &mut Cx, win: WindowId, cols: usize, rows: usize) {
        if let Some(Win::Term(t)) = self.windows.get_mut(&win) {
            t.snap_to = Some((cols, rows));
            cx.redraw(win);
        }
        self.settings.cols = cols;
        self.settings.rows = rows;
        self.settings.save();
    }

    /// The window and place of the tab running `shell`.
    fn find_shell(&self, shell: u64) -> Option<(WindowId, usize)> {
        self.windows.iter().find_map(|(&id, w)| match w {
            Win::Term(t) => t.position(shell).map(|i| (id, i)),
            _ => None,
        })
    }

    fn close(&mut self, cx: &mut Cx, win: WindowId) {
        if let Some(Win::Term(t)) = self.windows.remove(&win) {
            for tab in &t.tabs {
                tab.session.hang_up();
            }
        }
        cx.close_window(win);
        let finds: Vec<WindowId> =
            self.windows.iter().filter(|(_, w)| matches!(w, Win::Find { target, .. } if *target == win)).map(|(&id, _)| id).collect();
        for id in finds {
            self.windows.remove(&id);
            cx.close_window(id);
        }
        if !self.windows.values().any(|w| matches!(w, Win::Term(_))) {
            if std::mem::take(&mut self.respawn) {
                self.open(cx, None, None);
                return;
            }
            // Nothing to come back to.
            self.remember(cx, true);
            self.link.bye();
            cx.quit();
        }
    }

    /// Close the tab running `shell` (hanging it up); the window goes
    /// with its last tab.
    fn close_tab(&mut self, cx: &mut Cx, win: WindowId, shell: u64) {
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let Some(i) = t.position(shell) else { return };
        if t.tabs.len() == 1 {
            return self.close(cx, win);
        }
        t.take(i).session.hang_up();
        // Its group goes with its last tab.
        t.tidy();
        cx.redraw(win);
    }

    /// Close a tab, asking first if something is running in it. `why`
    /// (what asked for it) goes in the log with the question.
    fn close_tab_requested(&mut self, cx: &mut Cx, win: WindowId, shell: u64, why: &str) {
        let Some(Win::Term(t)) = self.windows.get(&win) else { return };
        let Some(i) = t.position(shell) else { return };
        let session = &t.tabs[i].session;
        match session.foreground().filter(|_| session.busy()) {
            Some(program) => {
                let place = if t.tabs.len() > 1 { "tab" } else { "window" };
                let message = format!("“{program}” is still running in this {place}. Close it anyway?");
                self.confirm_close(cx, win, Some(shell), message, why);
            }
            None => self.close_tab(cx, win, shell),
        }
    }

    /// Ask "Close it anyway?" about tab `shell` of `win` (None: the whole
    /// window), unless that's already being asked: then the question
    /// comes to the front again rather than another one over it.
    fn confirm_close(&mut self, cx: &mut Cx, win: WindowId, shell: Option<u64>, message: String, why: &str) {
        let asking = self.windows.iter().find_map(|(&id, w)| match w {
            Win::ConfirmClose { target, shell: s, .. } if *target == win && *s == shell => Some(id),
            _ => None,
        });
        if let Some(id) = asking {
            // Nobody asks twice on purpose: say where it came from.
            tracing::warn!(?shell, why, "asked again to close while asking already");
            return cx.activate(id);
        }
        tracing::info!(?shell, why, ask = %message, "asking before closing");
        let id = cx.open_window(WindowOptions::alert("Terminal", &message));
        self.windows.insert(id, Win::ConfirmClose { target: win, shell, message });
    }

    /// Close `window`, asking first if something is running in it.
    fn close_window_requested(&mut self, cx: &mut Cx, window: WindowId, why: &str) {
        match self.windows.get(&window) {
            Some(Win::Term(t)) => {
                let busy: Vec<String> = t.tabs.iter().filter(|x| x.session.busy()).filter_map(|x| x.session.foreground()).collect();
                let message = match busy.as_slice() {
                    [] => return self.close(cx, window),
                    [one] => format!("“{one}” is still running in this window. Close it anyway?"),
                    [first, rest @ ..] => format!(
                        "“{first}” and {} other program{} are still running in this window. Close it anyway?",
                        rest.len(),
                        if rest.len() == 1 { "" } else { "s" }
                    ),
                };
                self.confirm_close(cx, window, None, message, why);
            }
            Some(
                Win::ConfirmClose { .. }
                | Win::Find { .. }
                | Win::Daemon(_)
                | Win::ConfirmStartOver { .. }
                | Win::NameGroup { .. }
                | Win::ConfirmRemove { .. }
                | Win::Failed { .. },
            ) => {
                self.windows.remove(&window);
                cx.close_window(window);
            }
            Some(Win::Worktree(d)) => {
                d.stop();
                self.windows.remove(&window);
                cx.close_window(window);
            }
            Some(Win::Sudo(_)) => self.sudo_done(cx, window, Some(sudo_wire::Reply::Cancel)),
            None => cx.close_window(window),
        }
    }

    /// A tab's Add to Group menu: one of its window's groups (and first,
    /// in the tab's own menu, a new one: the menu bar has New Group…).
    fn add_to_group_menu(&self, win: WindowId, shell: u64, own: bool) -> Menu {
        let mut menu = Menu::new(if own { "Add to Group" } else { "Add Tab to Group" });
        if own {
            menu = menu.item("New Group…", format!("tab-new-group:{shell}"));
        }
        let Some(Win::Term(t)) = self.windows.get(&win) else { return menu };
        if t.groups.is_empty() {
            return if own { menu } else { menu.item("No Groups", "none").enabled(false) };
        }
        let current = t.position(shell).and_then(|i| t.tabs[i].group);
        if own {
            menu = menu.separator();
        }
        for (g, group) in t.groups.iter().enumerate() {
            menu = menu.item(&group.name, format!("tab-group:{shell}:{g}")).checked(current == Some(g));
        }
        menu
    }

    /// A tab's Move to Window menu: Terminal's other windows, by title.
    fn move_to_window_menu(&self, win: WindowId, shell: u64, own: bool) -> Menu {
        let mut menu = Menu::new(if own { "Move to Window" } else { "Move Tab to Window" });
        let mut others: Vec<(&String, u64)> = self
            .windows
            .iter()
            .filter(|(id, _)| **id != win)
            .filter_map(|(_, w)| match w {
                Win::Term(t) => t.tabs.first().map(|x| (&t.shown_title, x.session.id)),
                _ => None,
            })
            .collect();
        others.sort();
        for (title, target) in &others {
            menu = menu.item(title.as_str(), format!("tab-to:{shell}:{target}"));
        }
        if others.is_empty() {
            menu = menu.item("No Other Windows", "none").enabled(false);
        }
        menu
    }

    /// Group `g` of `win` into a window of its own, its tabs in it
    /// (`torn` off the strip: under the pointer).
    fn group_to_window(&mut self, cx: &mut Cx, win: WindowId, g: usize, torn: bool) -> Option<WindowId> {
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return None };
        let group = t.groups.get(g).cloned()?;
        if t.tabs.iter().all(|x| x.group == Some(g)) {
            return None;
        }
        let shown = t.tab().session.id;
        let (cols, rows) = t.grid();
        let mut tabs = Vec::new();
        while let Some(i) = t.tabs.iter().position(|x| x.group == Some(g)) {
            let mut tab = t.take(i);
            tab.group = Some(0);
            tabs.push(tab);
        }
        t.tidy();
        cx.redraw(win);
        let active = tabs.iter().position(|x| x.session.id == shown).unwrap_or(0);
        let mut options = self.window_options(cols, rows);
        options.under_pointer = torn;
        options.title = format!("{} — {}", group.name, tabs[active].title);
        let id = cx.open_window(options);
        self.windows.insert(id, Win::Term(Box::new(TermWin::new(tabs, active, Some((cols, rows)), vec![group]))));
        Some(id)
    }

    /// Window `window` let go on `onto`'s tab strip, at `place`: its tabs
    /// (and their groups) go in there, the one it showed showing, and it
    /// goes. Not inside one of `onto`'s groups unless it is that group.
    fn merge(&mut self, cx: &mut Cx, window: WindowId, onto: WindowId, place: usize) {
        if window == onto || !matches!(self.windows.get(&onto), Some(Win::Term(_))) {
            return;
        }
        let Some(Win::Term(from)) = self.windows.remove(&window) else { return };
        cx.close_window(window);
        let TermWin { tabs, groups, active, .. } = *from;
        let shown = tabs.get(active).map(|x| x.session.id);
        let Some(Win::Term(t)) = self.windows.get_mut(&onto) else { return };
        if t.focused {
            t.tab().report_focus(false);
        }
        let base = t.groups.len();
        t.groups.extend(groups);
        let mut at = place.min(t.tabs.len());
        // Not into the middle of a group of `onto`'s: after it.
        while at > 0 && at < t.tabs.len() && t.tabs[at].group.is_some() && t.tabs[at].group == t.tabs[at - 1].group {
            at += 1;
        }
        let n = tabs.len();
        for (k, mut tab) in tabs.into_iter().enumerate() {
            tab.group = tab.group.map(|g| g + base);
            tab.rang = false;
            t.tabs.insert(at + k, tab);
        }
        t.active = shown.and_then(|s| t.position(s)).unwrap_or(at.min(t.tabs.len() - n));
        if t.focused {
            t.tab().report_focus(true);
        }
        t.tidy();
        self.auto_group(cx, onto);
        cx.redraw(onto);
        cx.activate(onto);
    }

    /// Tab `shell` of `win` to the end of the window with tab `target`
    /// (out of its group: it may join its repository's there); `win`
    /// goes with its last tab.
    fn move_tab_to(&mut self, cx: &mut Cx, win: WindowId, shell: u64, target: u64) {
        let Some(to) = self.find_shell(target).map(|(w, _)| w).filter(|&w| w != win) else { return };
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let Some(i) = t.position(shell) else { return };
        let mut tab = if t.tabs.len() == 1 {
            // Its window goes, the shell with the tab.
            let Some(Win::Term(mut t)) = self.windows.remove(&win) else { return };
            cx.close_window(win);
            t.tabs.remove(0)
        } else {
            let tab = t.take(i);
            t.tidy();
            cx.redraw(win);
            tab
        };
        tab.group = None;
        tab.rang = false;
        let Some(Win::Term(t)) = self.windows.get_mut(&to) else { return };
        if t.focused {
            t.tab().report_focus(false);
        }
        t.tabs.push(tab);
        t.active = t.tabs.len() - 1;
        self.auto_group(cx, to);
        cx.redraw(to);
        cx.activate(to);
    }

    /// Tabs ▸ Group's commands, a group chip's and a tab's (`…:GROUP`,
    /// `…:SHELL`); true if `command` was one. `name-group`, `ungroup` and
    /// `save-group` are for the group of the tab showing.
    fn group_command(&mut self, cx: &mut Cx, win: WindowId, command: &str) -> bool {
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return false };
        let (verb, arg) = command.split_once(':').unwrap_or((command, ""));
        let number = |s: &str| s.parse::<u64>().ok();
        let showing = t.tab().group;
        let g = match verb {
            "name-group" | "ungroup" | "save-group" => showing,
            _ => number(arg).map(|g| g as usize),
        }
        .filter(|&g| g < t.groups.len());
        match verb {
            "name-group" => {
                let shell = t.tab().session.id;
                self.name_group(cx, win, g, shell);
            }
            "group-rename" if g.is_some() => self.name_group(cx, win, g, 0),
            "tab-new-group" => {
                if let Some(shell) = number(arg) {
                    self.name_group(cx, win, None, shell);
                }
            }
            "group-new-tab" => {
                let Some(g) = g else { return true };
                let cwd = t.members(g).last().and_then(|&i| t.tabs[i].session.cwd());
                self.add_tab_at(cx, win, Start::Spawn { command: None, cwd }, Some(g));
            }
            "save-group" | "group-save" => {
                let Some(g) = g else { return true };
                let members = t.members(g);
                let tabs = members.iter().map(|&i| groups::SavedTab { cwd: t.tabs[i].session.cwd(), title: t.tabs[i].title.clone() }).collect();
                let active = members.iter().position(|&i| i == t.active).unwrap_or(0);
                let saved = groups::Saved { group: t.groups[g].clone(), tabs, active };
                match self.saved_groups.iter_mut().find(|x| x.group.name == saved.group.name) {
                    Some(x) => *x = saved,
                    None => self.saved_groups.push(saved),
                }
                self.saved_groups.sort_by_key(|x| x.group.name.to_lowercase());
                groups::store(&self.saved_groups);
            }
            "ungroup" | "group-ungroup" => {
                let Some(g) = g else { return true };
                for tab in t.tabs.iter_mut().filter(|x| x.group == Some(g)) {
                    tab.group = None;
                    tab.own_grouping = true;
                }
                t.tidy();
                cx.redraw(win);
            }
            "group-close" => {
                let Some(g) = g else { return true };
                let shells: Vec<u64> = t.members(g).into_iter().map(|i| t.tabs[i].session.id).collect();
                for shell in shells {
                    self.close_tab_requested(cx, win, shell, "the group's menu ▸ Close Group");
                }
            }
            "group-window" => {
                if let Some(g) = g {
                    self.group_to_window(cx, win, g, false);
                }
            }
            "tab-group" => {
                let Some((shell, g)) = arg.split_once(':').and_then(|(a, b)| Some((number(a)?, number(b)? as usize))) else { return true };
                if let Some(i) = t.position(shell).filter(|_| g < t.groups.len()) {
                    t.tabs[i].group = Some(g);
                    t.tidy();
                    cx.redraw(win);
                }
            }
            "tab-ungroup" => {
                if let Some(i) = number(arg).and_then(|shell| t.position(shell)) {
                    t.tabs[i].group = None;
                    t.tabs[i].own_grouping = true;
                    t.tidy();
                    cx.redraw(win);
                }
            }
            "tab-to" => {
                if let Some((shell, target)) = arg.split_once(':').and_then(|(a, b)| Some((number(a)?, number(b)?))) {
                    self.move_tab_to(cx, win, shell, target);
                }
            }
            _ => return false,
        }
        true
    }

    /// Tab `shell` into a window of its own (`torn` off the strip: under
    /// the pointer).
    fn detach(&mut self, cx: &mut Cx, win: WindowId, shell: u64, torn: bool) -> Option<WindowId> {
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return None };
        let i = t.position(shell).filter(|_| t.tabs.len() > 1)?;
        let mut tab = t.take(i);
        cx.redraw(win);
        t.tidy();
        let (cols, rows) = (tab.session.size.cols, tab.session.size.rows);
        let mut options = self.window_options(cols, rows);
        options.under_pointer = torn;
        options.title = tab.title.clone();
        tab.rang = false;
        // Out of its group: it may join its repository's in its new window.
        tab.group = None;
        let id = cx.open_window(options);
        self.windows.insert(id, Win::Term(Box::new(TermWin::new(vec![tab], 0, Some((cols, rows)), Vec::new()))));
        self.auto_group(cx, id);
        Some(id)
    }
}

pub(crate) const START_OVER: &str = "Starting olo-termd over ends every shell in it, and whatever is running in them. Start over?";

const FIND_W: f32 = 340.0;
const FIND_H: f32 = 2.0 * m::WINDOW_INSET + m::CONTROL_HEIGHT + m::SPACING + 4.0 * m::MARK_ROW + 3.0 * m::TIGHT + m::SPACING + m::CONTROL_HEIGHT;

impl Terminal {
    fn open_find(&mut self, cx: &mut Cx, target: WindowId) {
        if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::Find { target: t, .. } if *t == target)) {
            cx.activate(id);
            return;
        }
        let (w, h) = (FIND_W as u32, FIND_H as u32);
        let field = Terminal::field(cx, &self.search.text, |f| f);
        let id = cx.open_window(WindowOptions::new("Find", w, h).min_size(w, h).max_size(900, h));
        ui::submit_on_enter(cx, &field, id, |t, cx, id| t.find_done(cx, id, true));
        let f = field.clone();
        cx.with_window(id, move |_, window, cx| f.read(cx).focus(window));
        self.windows.insert(id, Win::Find { target, field });
    }

    /// Search `win`'s scrollback (its tab showing); the visual bell says
    /// there's no match.
    fn find_in(&mut self, cx: &mut Cx, win: WindowId) {
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let found = find::find(&mut t.tab().session.term.lock(), &self.search);
        if !found {
            t.tab_mut().bell = Some(Instant::now());
        }
        cx.redraw(win);
    }
}

impl Terminal {
    /// A tab's agent started, stopped or moved on: show it, and say so
    /// when it now needs the person and they aren't looking at it.
    fn agent_changed(&mut self, cx: &mut Cx, agent::Report(shell, now): agent::Report) {
        let Some((win, i)) = self.find_shell(shell) else { return };
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let seen = t.focused && i == t.active;
        let tab = &mut t.tabs[i];
        let before = tab.agent;
        tab.agent = now;
        if now.is_some() {
            tab.agent_title_until = None;
        } else if tab.agent_title.take().is_some() {
            retitle(cx, win, t);
        }
        let tab = &mut t.tabs[i];
        cx.redraw(win);
        let wants = match (before, now) {
            (_, Some((agent, State::Blocked))) => Some((agent, State::Blocked)),
            (Some((_, State::Working | State::Blocked)), Some((agent, State::Idle))) => Some((agent, State::Idle)),
            _ => None,
        };
        let Some((agent, state)) = wants.filter(|_| !seen) else {
            // Answered or moving again: the note has had its day.
            if let Some(id) = self.notes.remove(&shell) {
                self.notifier.close(id);
            }
            return;
        };
        tab.attention = true;
        let place = match tab.group.and_then(|g| t.groups.get(g)) {
            Some(g) => format!("{} — {}", g.name, tab.title),
            None => tab.title.clone(),
        };
        let folder = tab.session.cwd().and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()));
        let body = match folder {
            Some(f) => format!("In “{place}”, in {f}."),
            None => format!("In “{place}”."),
        };
        let (summary, kind) = match state {
            State::Blocked => (format!("{} needs your answer", agent.name()), NoteKind::Important),
            _ => (format!("{} is waiting for you", agent.name()), NoteKind::Information),
        };
        let note = Note::new(summary).body(body).kind(kind).action("default", "Show");
        let id = self.notifier.show(self.notes.get(&shell).copied(), note);
        self.notes.insert(shell, id);
    }
}

const NAME_W: f32 = 340.0;
const NAME_H: f32 = 2.0 * m::WINDOW_INSET + m::CONTROL_HEIGHT + 2.0 * m::SPACING + ui::SWATCH + m::SPACING + m::CONTROL_HEIGHT;

impl Terminal {
    /// Saved group `i`: where it is open, its first tab; else at the end
    /// of `into` (the window it was chosen in), a shell in each of its
    /// tabs' folders; else in a window of its own.
    fn open_group(&mut self, cx: &mut Cx, i: usize, into: Option<WindowId>) {
        let Some(saved) = self.saved_groups.get(i).cloned() else { return };
        let open = self.windows.iter().find_map(|(&id, w)| match w {
            Win::Term(x) => x.groups.iter().position(|g| g.name == saved.group.name).map(|g| (id, g)),
            _ => None,
        });
        if let Some((id, g)) = open {
            if let Some(Win::Term(t)) = self.windows.get_mut(&id)
                && let Some(&first) = t.members(g).first()
            {
                t.select(first);
                cx.redraw(id);
            }
            return cx.activate(id);
        }
        let mut starts: Vec<Start> = saved.tabs.iter().map(|t| Start::Spawn { command: None, cwd: t.cwd.clone().filter(|d| d.is_dir()) }).collect();
        if starts.is_empty() {
            starts.push(Start::Spawn { command: None, cwd: None });
        }
        let n = starts.len();
        let Some((win, (cols, rows))) = into.and_then(|w| match self.windows.get(&w) {
            Some(Win::Term(t)) => Some((w, t.grid())),
            _ => None,
        }) else {
            let (cols, rows) = (self.settings.cols, self.settings.rows);
            let options = self.window_options(cols, rows);
            self.add(cx, options, Size { cols, rows }, Some((cols, rows)), starts, saved.active, (vec![saved.group], vec![(Some(0), false); n]));
            return;
        };
        let tabs: Vec<Tab> = starts.into_iter().map(|s| self.start(cx, Size { cols, rows }, s)).collect();
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let g = t.groups.len();
        t.groups.push(saved.group);
        let first = t.tabs.len();
        for mut tab in tabs {
            tab.group = Some(g);
            t.tabs.push(tab);
        }
        t.select(first + saved.active.min(n - 1));
        t.tidy();
        cx.redraw(win);
        cx.activate(win);
    }

    /// The Group window for `target`'s group `group`, else for a new group
    /// of its tab `tab`.
    fn name_group(&mut self, cx: &mut Cx, target: WindowId, group: Option<usize>, tab: u64) {
        let Some(Win::Term(t)) = self.windows.get(&target) else { return };
        let (name, color) = match group.and_then(|g| t.groups.get(g)) {
            Some(g) => (g.name.clone(), g.color),
            None => {
                let all: Vec<&Group> = self.windows.values().filter_map(|w| if let Win::Term(x) = w { Some(x) } else { None }).flat_map(|x| x.groups.iter()).collect();
                let names: Vec<&str> = all.iter().map(|g| g.name.as_str()).chain(self.saved_groups.iter().map(|g| g.group.name.as_str())).collect();
                let used: Vec<usize> = t.groups.iter().map(|g| g.color).collect();
                // In a repository: its name.
                let repo = t.position(tab).and_then(|i| t.tabs[i].repo.as_ref()).map(|r| r.name.clone());
                (repo.unwrap_or_else(|| groups::new_name(&names)), groups::new_color(&used))
            }
        };
        if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::NameGroup { target: t, .. } if *t == target)) {
            self.windows.remove(&id);
            cx.close_window(id);
        }
        let name = Terminal::field(cx, &name, |f| f);
        let id = cx.open_window(WindowOptions::new("Group", NAME_W as u32, NAME_H as u32).min_size(NAME_W as u32, NAME_H as u32).max_size(NAME_W as u32, NAME_H as u32));
        ui::submit_on_enter(cx, &name, id, |t, cx, id| t.name_group_done(cx, id, true));
        let f = name.clone();
        cx.with_window(id, move |_, window, cx| f.read(cx).focus(window));
        self.windows.insert(id, Win::NameGroup { target, group, tab, name, color });
    }

    /// The Group window answered: name (or make) the group, or not.
    pub(crate) fn name_group_done(&mut self, cx: &mut Cx, window: WindowId, ok: bool) {
        let Some(Win::NameGroup { name, color, group, tab, target }) = self.windows.get(&window) else { return };
        let (name, color, group, tab, target) = (name.read(cx).text().trim().to_owned(), *color, *group, *tab, *target);
        if ok && name.is_empty() {
            return;
        }
        self.windows.remove(&window);
        cx.close_window(window);
        if !ok {
            return;
        }
        let Some(Win::Term(t)) = self.windows.get_mut(&target) else { return };
        match group.filter(|&g| g < t.groups.len()) {
            Some(g) => {
                // A saved group renamed here keeps its saved tabs under the new name.
                if let Some(s) = self.saved_groups.iter_mut().find(|s| s.group.name == t.groups[g].name) {
                    s.group.name = name.clone();
                    s.group.color = color;
                    groups::store(&self.saved_groups);
                }
                let g = &mut t.groups[g];
                (g.name, g.color, g.auto) = (name, color, false);
            }
            None => {
                let Some(i) = t.position(tab) else { return };
                // Its repository's, if it's in one (its worktrees in the group's menu).
                let repo = t.tabs[i].repo.as_ref().map(|r| r.main.clone());
                t.groups.push(Group { name, color, repo, auto: false });
                t.tabs[i].group = Some(t.groups.len() - 1);
                t.tidy();
            }
        }
        cx.redraw(target);
        cx.activate(target);
    }
}

// ---- sudo's password (sudo.rs) ---------------------------------------------

impl Terminal {
    /// The tab process `pid` runs in (its shell is an ancestor), and the
    /// coding agent that started it, if one did.
    fn requester(&self, pid: i32) -> (Option<(WindowId, usize)>, Option<Agent>) {
        let mut agent = None;
        for p in sudo::ancestors(pid) {
            let tab = self.windows.iter().find_map(|(&id, w)| match w {
                Win::Term(t) => t.tabs.iter().position(|x| x.session.shared.shell() == Some(p)).map(|i| (id, i)),
                _ => None,
            });
            if tab.is_some() {
                return (tab, agent);
            }
            agent = agent.or_else(|| agent::which(p));
        }
        (None, agent)
    }

    fn sudo_window(&self, conn: u64) -> Option<WindowId> {
        self.windows.iter().find_map(|(&id, w)| matches!(w, Win::Sudo(r) if r.conn == conn).then_some(id))
    }

    fn sudo_event(&mut self, cx: &mut Cx, e: sudo::Event) {
        match e {
            sudo::Event::Begin { conn, pid, stream, args, tty, approval } => {
                let (tab, who) = self.requester(pid);
                let title = tab.and_then(|(id, i)| match self.windows.get(&id) {
                    Some(Win::Term(t)) => Some(t.tabs[i].current_title()),
                    _ => None,
                });
                let mut r = sudo::Request::new(conn, stream, &args, who, title);
                // Typed at a shell, with the window turned off: there.
                if tty && !self.settings.sudo_window {
                    return r.reply(&sudo_wire::Reply::Pass);
                }
                r.reply(&sudo_wire::Reply::Go);
                // Approved without asking: sudo gets the password when
                // it asks for it.
                if approval.is_some() {
                    r.approval = approval;
                    tracing::info!(command = ?args, agent = ?who.map(Agent::name), "sudo approved without asking");
                    self.sudo_approved.insert(conn, (Box::new(r), tab.map(|(id, _)| id)));
                    return;
                }
                tracing::info!(command = ?args, agent = ?who.map(Agent::name), "sudo asks for a password");
                self.sudo_ask(cx, r, tab.map(|(id, _)| id));
            }
            sudo::Event::Said(conn, said) if self.sudo_approved.contains_key(&conn) => {
                let Some((mut r, over)) = self.sudo_approved.remove(&conn) else { return };
                match r.approved(said) {
                    sudo::Approved::Waiting => {
                        self.sudo_approved.insert(conn, (r, over));
                    }
                    sudo::Approved::Done(ok) => {
                        let what = r.summary();
                        tracing::info!(ok, what, "sudo approved without asking: done");
                        // Quietly, so it doesn't pass unseen.
                        let title = if ok { "Approved without asking" } else { "sudo said no" };
                        let mut note = Note::new(title).body(what);
                        note.silent = true;
                        self.notifier.show(None, note);
                    }
                    sudo::Approved::AskPerson => {
                        tracing::warn!(what = r.summary(), "sudo approved without asking: the password didn't do; asking");
                        self.sudo_ask(cx, *r, over);
                    }
                }
            }
            sudo::Event::Said(conn, said) => {
                let Some(id) = self.sudo_window(conn) else { return };
                let Some(Win::Sudo(r)) = self.windows.get_mut(&id) else { return };
                if r.said(said) {
                    self.sudo_done(cx, id, None);
                } else {
                    cx.redraw(id);
                }
            }
            sudo::Event::Touch(conn) => {
                if let Some(id) = self.sudo_window(conn)
                    && let Some(Win::Sudo(r)) = self.windows.get_mut(&id)
                {
                    r.touch();
                    cx.redraw(id);
                }
            }
            sudo::Event::Gone(conn) => {
                self.sudo_approved.remove(&conn);
                if let Some(id) = self.sudo_window(conn) {
                    self.sudo_done(cx, id, None);
                }
            }
        }
    }

    /// Ask the person for sudo's password, in an alert over `over` (else
    /// the front window).
    fn sudo_ask(&mut self, cx: &mut Cx, mut r: sudo::Request, over: Option<WindowId>) {
        let size = (sudo::WIDTH, r.height(cx));
        let _ = over;
        let id = cx.open_window(WindowOptions::new("sudo", size.0, size.1).min_size(size.0, size.1).max_size(size.0, size.1));
        r.focus_field(cx, id);
        self.windows.insert(id, Win::Sudo(Box::new(r)));
        cx.activate(id);
    }

    /// Close a sudo window, telling sudo `reply` first.
    fn sudo_done(&mut self, cx: &mut Cx, window: WindowId, reply: Option<sudo_wire::Reply>) {
        if let Some(Win::Sudo(mut r)) = self.windows.remove(&window) {
            if let Some(reply) = reply {
                r.reply(&reply);
            }
            r.clear(cx);
        }
        cx.close_window(window);
    }
}

impl Terminal {
    /// Button `choice` of alert window `window` (Return, Escape, a click).
    pub(crate) fn alert_chosen(&mut self, cx: &mut Cx, window: WindowId, choice: usize) {
        match self.windows.get(&window) {
            Some(&Win::ConfirmClose { target, shell, .. }) => {
                self.windows.remove(&window);
                cx.close_window(window);
                if choice == 1 {
                    match shell {
                        Some(shell) => self.close_tab(cx, target, shell),
                        None => self.close(cx, target),
                    }
                }
            }
            Some(Win::ConfirmRemove { .. }) => {
                let Some(Win::ConfirmRemove { main, path, .. }) = self.windows.remove(&window) else { return };
                cx.close_window(window);
                if choice == 1 {
                    self.remove_worktree(cx, main, path);
                }
            }
            Some(Win::Failed { .. }) => {
                self.windows.remove(&window);
                cx.close_window(window);
            }
            Some(&Win::ConfirmStartOver { target }) => {
                self.windows.remove(&window);
                cx.close_window(window);
                if choice == 1
                    && let Some(Win::Daemon(d)) = self.windows.get_mut(&target)
                {
                    self.respawn = true;
                    // Its shells go for good: not kept for next time.
                    self.link.starting_over();
                    d.start_over(cx.proxy());
                }
            }
            _ => {}
        }
    }

    /// What was done in the Terminal Daemon window.
    pub(crate) fn daemon_action(&mut self, cx: &mut Cx, window: WindowId, action: daemon::Action) {
        match action {
            daemon::Action::ConfirmStartOver => {
                let id = cx.open_window(WindowOptions::alert("Terminal", START_OVER));
                self.windows.insert(id, Win::ConfirmStartOver { target: window });
            }
            daemon::Action::Show(shell) => match self.find_shell(shell.id) {
                Some((id, i)) => {
                    if let Some(Win::Term(t)) = self.windows.get_mut(&id) {
                        t.select(i);
                    }
                    cx.activate(id);
                    cx.redraw(id);
                }
                None => {
                    self.reopen(cx, vec![Start::Attach(shell.id)], Some(&shell), None, 0, Default::default());
                }
            },
        }
    }

    /// The sudo window's answer: its password, or no.
    pub(crate) fn sudo_choice(&mut self, cx: &mut Cx, window: WindowId, choice: sudo::Choice) {
        match choice {
            sudo::Choice::Authenticate => {
                if let Some(Win::Sudo(r)) = self.windows.get_mut(&window) {
                    r.answer(cx);
                    cx.redraw(window);
                }
            }
            sudo::Choice::Cancel => self.sudo_done(cx, window, Some(sudo_wire::Reply::Cancel)),
        }
    }

    /// Files dropped on a terminal are typed as their paths (quoted for
    /// the shell, as macOS's Terminal does), text as if pasted.
    pub(crate) fn dropped(&mut self, cx: &mut Cx, window: WindowId, paths: &[PathBuf], text: Option<&str>) {
        let Some(Win::Term(t)) = self.windows.get(&window) else { return };
        let text = if paths.is_empty() { text.unwrap_or_default().to_owned() } else { paths.iter().map(|p| shell_quote(&p.to_string_lossy())).collect::<Vec<_>>().join(" ") + " " };
        view::paste(&t.tab().session, &text);
        cx.activate(window);
    }

    pub(crate) fn menus(&self, window: WindowId) -> Option<Vec<Menu>> {
        let Some(Win::Term(t)) = self.windows.get(&window) else { return None };
        let has_selection = t.tab().session.has_selection();
        let size = t.grid();
        let several = t.tabs.len() > 1;
        let mut window_size = Menu::new("Window Size");
        for (c, r) in [(80, 24), (80, 25), (80, 40), (120, 25), (120, 40), (132, 25), (132, 50)] {
            window_size = window_size.item(format!("{c} × {r}"), format!("grid-{c}x{r}")).checked(size == (c, r));
        }
        let mut text = Menu::new("Text Size")
            .item("Bigger", "bigger").shift_key("=")
            .item("Smaller", "smaller").shift_key("-")
            .item("Default", "default-size").shift_key("0")
            .separator();
        for s in TEXT_SIZES {
            text = text.item(format!("{s}"), format!("size-{s}")).checked(self.settings.text_size == s);
        }
        // The look's own colours, BeOS's and Catppuccin's for each mode
        // (and the one chosen, if it is none of those).
        let pick = |title: &str, mode: ThemeMode| {
            let chosen = self.colors_for(mode);
            let mut offered = looks::terminals(&self.look.theme, mode);
            if !offered.contains(&chosen) {
                offered.push(chosen.clone());
            }
            let slot = if mode == ThemeMode::Dark { "dark" } else { "light" };
            offered.iter().fold(Menu::new(title), |m, t| m.item(&t.name, format!("colors-{slot}:{}", t.id)).checked(t.id == chosen.id))
        };
        let scheme = Menu::new("Colors")
            .item("Match Appearance", "scheme-auto")
            .checked(self.settings.scheme == SchemeChoice::Auto)
            .item("Always Light", "scheme-light")
            .checked(self.settings.scheme == SchemeChoice::Light)
            .item("Always Dark", "scheme-dark")
            .checked(self.settings.scheme == SchemeChoice::Dark)
            .separator()
            .submenu(pick("Light Colors", ThemeMode::Light))
            .submenu(pick("Dark Colors", ThemeMode::Dark));
        const NUMBERS: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];
        let shell = t.tab().session.id;
        let in_group = t.tab().group;
        let mut open_group = Menu::new("Open Saved Group");
        let mut forget_group = Menu::new("Forget Saved Group");
        for (i, g) in self.saved_groups.iter().enumerate() {
            let open = self.windows.values().any(|w| matches!(w, Win::Term(x) if x.groups.iter().any(|x| x.name == g.group.name)));
            open_group = open_group.item(&g.group.name, format!("open-group:{i}")).checked(open);
            forget_group = forget_group.item(&g.group.name, format!("forget-group:{i}"));
        }
        if self.saved_groups.is_empty() {
            open_group = open_group.item("No Saved Groups", "none").enabled(false);
            forget_group = forget_group.item("No Saved Groups", "none").enabled(false);
        }
        // The group of the tab showing.
        let of = |command: &str| in_group.map_or_else(|| "none".to_owned(), |g| format!("{command}:{g}"));
        let alone = in_group.is_some_and(|g| t.tabs.iter().all(|x| x.group == Some(g)));
        let group = Menu::new("Group")
            .item("New Group…", format!("tab-new-group:{shell}"))
            .submenu(self.add_to_group_menu(window, shell, false))
            .item("Remove Tab from Group", format!("tab-ungroup:{shell}"))
            .enabled(in_group.is_some())
            .separator()
            .item("Rename Group…", of("group-rename"))
            .enabled(in_group.is_some())
            .item("New Tab in Group", of("group-new-tab"))
            .enabled(in_group.is_some())
            .item("Save Group", of("group-save"))
            .enabled(in_group.is_some())
            .item("Move Group to New Window", of("group-window"))
            .enabled(in_group.is_some() && !alone)
            .item("Ungroup", of("group-ungroup"))
            .enabled(in_group.is_some())
            .item("Close Group", of("group-close"))
            .enabled(in_group.is_some())
            .separator()
            .submenu(open_group)
            .submenu(forget_group);
        let mut tabs = Menu::new("Tabs")
            .item("Previous Tab", "prev-tab").shift_key("[")
            .enabled(several)
            .item("Next Tab", "next-tab").shift_key("]")
            .enabled(several)
            .separator()
            .item("Move Tab to New Window", "detach")
            .enabled(several)
            .submenu(self.move_to_window_menu(window, shell, false))
            .separator()
            .submenu(group)
            .submenu(self.repository_menu(window))
            .item("Tile All Tabs", "tile-tabs").shift_key("d")
            .enabled(several)
            .checked(t.tiled && several)
            .separator();
        for (i, tab) in t.tabs.iter().enumerate() {
            let label = match (tab.agent, tab.rang) {
                (Some((agent, state)), _) => format!("{} — {} {}", tab.title, agent.name(), state.describe()),
                (None, true) => format!("{} (bell)", tab.title),
                (None, false) => tab.title.clone(),
            };
            tabs = match NUMBERS.get(i) {
                Some(&key) => tabs.item(label, format!("tab:{i}")).shift_key(key),
                None => tabs.item(label, format!("tab:{i}")),
            }
            .checked(i == t.active);
        }
        // (Their keys are Terminal's in the keyboard profile, olo_protocol's
        // app_keys: Ctrl+Shift with any other key goes to the shell.)
        Some(vec![
            Menu::new("Terminal")
                .item("New Terminal", "new").shift_key("n")
                .item("New Tab", "new-tab").shift_key("t")
                .separator()
                .item(olo_kit_base::tracker::LABEL, olo_kit_base::tracker::COMMAND)
                .separator()
                .item("Terminal Daemon…", "daemon")
                .separator()
                .item(if several { "Close Tab" } else { "Close" }, "close").shift_key("w")
                .item("Close Window", "close-window")
                .enabled(several)
                .item("Quit", "quit").shift_key("q"),
            Menu::new("Edit")
                .item("Copy", "copy").shift_key("c")
                .enabled(has_selection)
                .item("Paste", "paste").shift_key("v")
                .separator()
                .item("Select All", "select-all").shift_key("a")
                .separator()
                .item("Find…", "find").shift_key("f")
                .item("Find Again", "find-again").shift_key("g")
                .enabled(!self.search.text.is_empty())
                .item("Find Selection", "find-selection").shift_key("e")
                .enabled(has_selection)
                .separator()
                .item("Clear Scrollback", "clear").shift_key("k"),
            tabs,
            Menu::new("Settings").submenu(window_size).submenu(text).submenu(scheme).submenu(
                Menu::new("Tabs")
                    .item("Along the Top", "tabs-top")
                    .checked(!self.settings.side_tabs)
                    .item("Down the Side", "tabs-side")
                    .checked(self.settings.side_tabs),
            )
            .separator()
            .item("Ask for sudo Passwords in a Window", "sudo-window")
            .checked(self.settings.sudo_window),
        ])
    }

    pub(crate) fn command(&mut self, cx: &mut Cx, window: WindowId, command: &str) {
        if !matches!(self.windows.get(&window), Some(Win::Term(_))) || self.repo_command(cx, window, command) || self.group_command(cx, window, command) {
            return;
        }
        let Some(Win::Term(t)) = self.windows.get_mut(&window) else { return };
        match command {
            "new" => {
                let cwd = t.tab().session.cwd();
                self.open(cx, cwd, None);
            }
            "new-tab" => {
                let cwd = t.tab().session.cwd();
                self.add_tab(cx, window, Start::Spawn { command: None, cwd });
            }
            "close" => {
                let shell = t.tab().session.id;
                self.close_tab_requested(cx, window, shell, "Terminal ▸ Close Tab (⌘W)");
            }
            "close-window" => self.close_window_requested(cx, window, "Terminal ▸ Close Window"),
            "next-tab" | "prev-tab" => {
                let n = t.tabs.len();
                let i = if command == "next-tab" { (t.active + 1) % n } else { (t.active + n - 1) % n };
                t.select(i);
                cx.redraw(window);
            }
            "detach" => {
                let shell = t.tab().session.id;
                self.detach(cx, window, shell, false);
            }
            "tile-reset-heights" => {
                if let Some(Win::Term(t)) = self.windows.get_mut(&window) {
                    t.tile_heights.clear();
                    t.tile_widths.clear();
                    cx.redraw(window);
                }
            }
            "tile-tabs" => {
                if let Some(Win::Term(t)) = self.windows.get_mut(&window) {
                    t.tiled = !t.tiled;
                    if !t.tiled {
                        // Back to one at a time: each tab takes the window's grid.
                        t.snap_to = None;
                    }
                    cx.redraw(window);
                }
            }
            "quit" => {
                // The shells stay in olo-termd for next time, and the
                // windows on disk.
                self.remember(cx, true);
                self.link.bye();
                for id in self.windows.keys().copied().collect::<Vec<_>>() {
                    cx.close_window(id);
                }
                self.windows.clear();
                cx.quit();
            }
            "copy" => {
                let text = t.tab().session.term.lock().selection_to_string();
                if let Some(text) = text {
                    cx.copy(text);
                }
            }
            "paste" => self.paste(cx, window),
            "select-all" => {
                use alacritty_terminal::{
                    index::{Column, Line, Point, Side},
                    selection::{Selection, SelectionType},
                };
                let mut term = t.tab().session.term.lock();
                let top = Point::new(Line(-(term.history_size() as i32)), Column(0));
                let bottom = Point::new(Line(term.screen_lines() as i32 - 1), term.last_column());
                let mut sel = Selection::new(SelectionType::Simple, top, Side::Left);
                sel.update(bottom, Side::Right);
                term.selection = Some(sel);
                drop(term);
                cx.redraw(window);
            }
            "find" => self.open_find(cx, window),
            "daemon" => {
                if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::Daemon(_))) {
                    cx.activate(id);
                } else {
                    let (id, d) = daemon::open(cx);
                    self.windows.insert(id, Win::Daemon(Box::new(d)));
                }
            }
            "find-again" => self.find_in(cx, window),
            "find-selection" => {
                let text = t.tab().session.term.lock().selection_to_string();
                if let Some(text) = text {
                    // One line: a match never crosses one.
                    self.search.text = text.lines().next().unwrap_or_default().to_owned();
                    self.find_in(cx, window);
                }
            }
            "clear" => {
                t.tab().session.clear_history();
                cx.redraw(window);
            }
            olo_kit_base::tracker::COMMAND => open_in_tracker(t.tab()),
            "bigger" | "smaller" | "default-size" => {
                let cur = self.settings.text_size;
                let next = match command {
                    "bigger" => TEXT_SIZES.iter().copied().find(|&s| s > cur).unwrap_or(cur),
                    "smaller" => TEXT_SIZES.iter().rev().copied().find(|&s| s < cur).unwrap_or(cur),
                    _ => DEFAULT_TEXT_SIZE,
                };
                self.set_text_size(cx, next);
            }
            c if c.starts_with("size-") => {
                if let Ok(s) = c[5..].parse::<f32>() {
                    self.set_text_size(cx, s);
                }
            }
            c if c.starts_with("grid-") => {
                if let Some((cols, rows)) = c[5..].split_once('x')
                    && let (Ok(cols), Ok(rows)) = (cols.parse(), rows.parse())
                {
                    self.set_grid(cx, window, cols, rows);
                }
            }
            "sudo-window" => {
                self.settings.sudo_window = !self.settings.sudo_window;
                sudo::WINDOW_FOR_TYPED.store(self.settings.sudo_window, std::sync::atomic::Ordering::Relaxed);
                self.settings.save();
            }
            "tabs-top" | "tabs-side" => {
                // The windows keep their sizes; their grids make room.
                self.settings.side_tabs = command == "tabs-side";
                self.settings.save();
                cx.redraw_all();
            }
            "scheme-auto" | "scheme-light" | "scheme-dark" => {
                self.settings.scheme = match command {
                    "scheme-light" => SchemeChoice::Light,
                    "scheme-dark" => SchemeChoice::Dark,
                    _ => SchemeChoice::Auto,
                };
                self.settings.save();
                cx.redraw_all();
            }
            c if let Some((slot, id)) = c.strip_prefix("colors-").and_then(|c| c.split_once(':'))
                && looks::terminal(id).is_some() =>
            {
                let id = Some(id.to_owned());
                if slot == "dark" {
                    self.settings.dark_colors = id;
                } else {
                    self.settings.light_colors = id;
                }
                self.settings.save();
                cx.redraw_all();
            }
            c => {
                if let Some(i) = c.strip_prefix("tab:").and_then(|n| n.parse().ok()) {
                    t.select(i);
                    cx.redraw(window);
                } else if let Some(shell) = c.strip_prefix("tab-close:").and_then(|n| n.parse().ok()) {
                    self.close_tab_requested(cx, window, shell, "the tab's menu ▸ Close Tab");
                } else if let Some(shell) = c.strip_prefix("tab-detach:").and_then(|n| n.parse().ok()) {
                    self.detach(cx, window, shell, false);
                } else if let Some(shell) = c.strip_prefix(olo_kit_base::tracker::COMMAND).and_then(|n| n.strip_prefix(':')?.parse::<u64>().ok()) {
                    if let Some(tab) = t.position(shell).map(|i| &t.tabs[i]) {
                        open_in_tracker(tab);
                    }
                } else if let Some(i) = c.strip_prefix("open-group:").and_then(|n| n.parse::<usize>().ok()) {
                    self.open_group(cx, i, Some(window));
                } else if let Some(i) = c.strip_prefix("forget-group:").and_then(|n| n.parse::<usize>().ok()) {
                    if i < self.saved_groups.len() {
                        self.saved_groups.remove(i);
                        groups::store(&self.saved_groups);
                    }
                } else if let Some(keep) = c.strip_prefix("tab-close-others:").and_then(|n| n.parse::<u64>().ok()) {
                    let others: Vec<u64> = t.tabs.iter().map(|x| x.session.id).filter(|&s| s != keep).collect();
                    for shell in others {
                        self.close_tab_requested(cx, window, shell, "the tab's menu ▸ Close Other Tabs");
                    }
                }
            }
        }
    }

    pub(crate) fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        // The window's close box, or another program (app_server logs which).
        self.close_window_requested(cx, window, "app_server");
    }

    pub(crate) fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) => return self.handle_launch(cx, *launch),
            Err(m) => m,
        };
        let message = match message.downcast::<AgentContext>() {
            Ok(on) => return self.set_agent_context(on.groups, on.titles),
            Err(m) => m,
        };
        let message = match message.downcast::<daemon::Report>() {
            Ok(report) => {
                for w in self.windows.values_mut() {
                    if let Win::Daemon(d) = w {
                        d.report(*report);
                        break;
                    }
                }
                cx.redraw_all();
                return;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<sudo::Event>() {
            Ok(event) => return self.sudo_event(cx, *event),
            Err(m) => m,
        };
        let message = match message.downcast::<agent::Report>() {
            Ok(report) => return self.agent_changed(cx, *report),
            Err(m) => m,
        };
        let Some(message) = self.repo_message(cx, message) else { return };
        let message = match message.downcast::<NoteEvent>() {
            Ok(event) => {
                if let NoteEvent::Action { id, token, .. } = *event
                    && let Some((&shell, _)) = self.notes.iter().find(|(_, n)| **n == id)
                    && let Some((win, i)) = self.find_shell(shell)
                {
                    if let Some(Win::Term(t)) = self.windows.get_mut(&win) {
                        t.select(i);
                    }
                    cx.activate_with(win, token);
                    cx.redraw(win);
                }
                return;
            }
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<Message>() else { return };
        let Message(shell, event) = *msg;
        let Some((win, i)) = self.find_shell(shell) else { return };
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let showing = i == t.active;
        let tab = &mut t.tabs[i];
        match event {
            // A tab that isn't showing is drawn (and woken again) when it is.
            TermEvent::Wakeup => {
                if showing {
                    cx.redraw(win);
                }
                self.retitle_soon(cx);
            }
            TermEvent::Title(title) => {
                tab.session.title = title;
                // At once, not at its next frame: a window not shown (behind
                // another in its tab group, covered) is only let draw once a
                // second, and its tab (an agent's status in the title) would
                // lag that much.
                retitle(cx, win, t);
                cx.redraw(win);
            }
            TermEvent::Bell => {
                if showing {
                    tab.bell = Some(Instant::now());
                } else {
                    tab.rang = true;
                }
                cx.redraw(win);
            }
            TermEvent::Clipboard(text) => cx.copy(text),
            TermEvent::Exited(code) => {
                tab.session.exited = Some(code);
                // As on BeOS: the window goes when its (last) shell does.
                self.close_tab(cx, win, shell);
            }
        }
    }

    pub(crate) fn save_state(&self, cx: &App) -> Option<String> {
        serde_json::to_string(&self.layout(cx)).ok()
    }

    pub(crate) fn window_dropped(&mut self, cx: &mut Cx, window: WindowId, onto: WindowId, place: Option<usize>) {
        if let Some(place) = place {
            self.merge(cx, window, onto, place);
        }
    }
}

impl Terminal {
    /// Each window's shells, in order, and where they are.
    fn layout(&self, cx: &App) -> Saved {
        let mut ids: Vec<WindowId> = self.windows.keys().copied().collect();
        ids.sort();
        let windows = ids
            .into_iter()
            .filter_map(|id| match self.windows.get(&id) {
                Some(Win::Term(t)) => {
                    let live: Vec<&Tab> = t.tabs.iter().filter(|x| x.session.exited.is_none()).collect();
                    let tabs: Vec<u64> = live.iter().map(|x| x.session.id).collect();
                    let shell = t.tab().session.id;
                    let active = tabs.iter().position(|&s| s == shell).unwrap_or(0);
                    (!tabs.is_empty()).then(|| SavedWindow {
                        shell: tabs[active],
                        tabs,
                        active,
                        size: ui::window_size(cx, id).unwrap_or((640, 400)),
                        group: None,
                        tiled: t.tiled,
                        tile_groups: t.tile_groups.clone(),
                        tile_heights: t.tile_heights.clone(),
                        tile_widths: t.tile_widths.clone(),
                        cwds: live.iter().map(|x| x.session.cwd()).collect(),
                        own_grouping: false,
                        groups: t.groups.clone(),
                        tab_groups: live.iter().map(|x| x.group).collect(),
                        own: live.iter().map(|x| x.own_grouping).collect(),
                        agent_titles: if live.iter().any(|x| x.agent_title.is_some()) { live.iter().map(|x| x.agent_title.clone()).collect() } else { Vec::new() },
                    })
                }
                _ => None,
            })
            .collect();
        Saved { windows }
    }

    /// Keep the windows as they are on disk, so that the next Terminal
    /// (after Quit, a crash, or in the next session) comes back with every
    /// window, group, tab and folder. `now`: before quitting, rather than
    /// on a thread of its own. Not once the session is ending: they're
    /// kept as they were, while their shells go.
    /// Preferences ▸ Terminal's agent settings: the agents' hook (either)
    /// and title tool (`titles`) put in or taken out, off the UI thread,
    /// and their file kept or not.
    fn set_agent_context(&mut self, groups: bool, titles: bool) {
        let was = self.agent_context;
        if was == Some((groups, titles)) {
            return;
        }
        self.agent_context = Some((groups, titles));
        self.agent_snapshot = None;
        let on = groups || titles;
        let hooks = was.is_none_or(|(g, t)| (g || t) != on);
        let tool = was.is_none_or(|(_, t)| t != titles);
        let exe = std::env::current_exe().ok().map(|e| e.to_str().and_then(|s| s.strip_suffix(" (deleted)")).map(PathBuf::from).unwrap_or(e));
        std::thread::spawn(move || {
            if !on {
                agent_context::clear();
            }
            let Some(exe) = exe else { return };
            if hooks && let Err(e) = agent_context::set_hooks(on, &exe) {
                tracing::warn!("coding agents' hook: {e}");
            }
            if tool && let Err(e) = agent_title::register(titles, &exe) {
                tracing::warn!("coding agents' tab title tool: {e}");
            }
        });
        if on {
            self.keep_agent_snapshot();
        }
    }

    /// Titles back from before a restart whose agents weren't seen again.
    fn expire_agent_titles(&mut self, cx: &mut Cx) {
        let now = Instant::now();
        for (&win, w) in self.windows.iter_mut() {
            let Win::Term(t) = w else { continue };
            let mut changed = false;
            for tab in t.tabs.iter_mut().filter(|tab| tab.agent_title_until.is_some_and(|u| u <= now)) {
                tab.agent_title_until = None;
                if tab.agent.is_none() {
                    changed |= tab.agent_title.take().is_some();
                }
            }
            if changed {
                retitle(cx, win, t);
                cx.redraw(win);
            }
        }
    }

    /// A coding agent's title for the tab whose shell is `pid`
    /// (agent_title.rs; empty: the tab's own again).
    fn set_agent_title(&mut self, cx: &mut Cx, pid: i32, title: &str) {
        let title = agent_title::tidy(title);
        for (&win, w) in self.windows.iter_mut() {
            let Win::Term(t) = w else { continue };
            let Some(tab) = t.tabs.iter_mut().find(|tab| tab.session.shared.shell() == Some(pid)) else { continue };
            tab.agent_title = (!title.is_empty()).then_some(title);
            retitle(cx, win, t);
            cx.redraw(win);
            return;
        }
    }

    /// While coding agents are told about their groups: each tab's shell,
    /// group, folder and agent, kept for the hook when they change.
    fn keep_agent_snapshot(&mut self) {
        let Some((groups, titles)) = self.agent_context.filter(|(g, t)| *g || *t) else { return };
        let display = olo_xdg::display_name();
        let tabs = self
            .windows
            .values()
            .filter_map(|w| match w {
                Win::Term(t) => Some(t),
                _ => None,
            })
            .flat_map(|t| {
                t.tabs.iter().map(|tab| agent_context::TabInfo {
                    shell: tab.session.shared.shell(),
                    group: tab.group.and_then(|g| t.groups.get(g)).map(|g| g.name.clone()),
                    cwd: tab.session.cwd(),
                    agent: tab.agent.map(|(a, _)| a.name().to_owned()),
                    display: Some(display.clone()),
                })
            })
            .collect();
        let snapshot = agent_context::Snapshot { tabs, groups, titles };
        if self.agent_snapshot.as_ref() == Some(&snapshot) {
            return;
        }
        self.agent_snapshot = Some(snapshot.clone());
        std::thread::spawn(move || {
            if let Err(e) = agent_context::write(&snapshot) {
                tracing::warn!("coding agents' tab groups: {e}");
            }
        });
    }

    /// Something was typed: the titles are looked at again soon (and
    /// the windows kept).
    pub(crate) fn typed(&mut self, cx: &mut Cx) {
        self.retitle_soon(cx);
    }

    /// The tabs' titles looked at again soon: once for a burst of output
    /// or typing (a program started may write nothing; its name is the
    /// title), not on a timer.
    pub(crate) fn retitle_soon(&mut self, cx: &mut Cx) {
        if std::mem::replace(&mut self.retitling, true) {
            return;
        }
        cx.spawn(async move |me, cx| {
            cx.background_executor().timer(RETITLE_AFTER).await;
            let _ = me.update(cx, |t, cx| {
                t.retitling = false;
                let ids: Vec<WindowId> = t.windows.keys().copied().collect();
                for id in ids {
                    if let Some(Win::Term(w)) = t.windows.get_mut(&id) {
                        retitle(cx, id, w);
                    }
                }
                t.remember_soon(cx);
            });
        })
        .detach();
    }

    /// The windows kept on disk soon: once for a burst of changes.
    pub(crate) fn remember_soon(&mut self, cx: &mut Cx) {
        if std::mem::replace(&mut self.remembering, true) {
            return;
        }
        cx.spawn(async move |me, cx| {
            cx.background_executor().timer(REMEMBER_AFTER).await;
            let _ = me.update(cx, |t, cx| {
                t.remembering = false;
                t.keep_agent_snapshot();
                t.remember(cx, false);
            });
        })
        .detach();
    }

    fn remember(&mut self, cx: &App, now: bool) {
        if self.link.ended() {
            return;
        }
        let Ok(text) = serde_json::to_string(&self.layout(cx)) else { return };
        if self.remembered.as_deref() == Some(text.as_str()) {
            return;
        }
        self.remembered = Some(text.clone());
        let write = move || {
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
            let path = layout_path();
            if let Some(dir) = path.parent() {
                let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir);
            }
            let new = path.with_extension("json.new");
            let file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&new);
            if file.and_then(|mut f| std::io::Write::write_all(&mut f, text.as_bytes())).is_ok() {
                let _ = std::fs::rename(&new, &path);
            }
        };
        if now {
            write();
        } else {
            std::thread::spawn(write);
        }
    }
}

fn main() {
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // A coding agent's hook (Preferences ▸ Terminal): what it's told.
    if args.first().map(String::as_str) == Some(agent_context::FLAG) {
        std::process::exit(match agent_context::run() {
            c if c == std::process::ExitCode::SUCCESS => 0,
            _ => 1,
        });
    }
    // A coding agent's tab title tool (Preferences ▸ Terminal).
    if args.first().map(String::as_str) == Some(agent_title::MCP_FLAG) {
        std::process::exit(match agent_title::serve() {
            c if c == std::process::ExitCode::SUCCESS => 0,
            _ => 1,
        });
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    if args.first().map(String::as_str) == Some(agent_title::SET_FLAG) {
        eprintln!("olo-terminal: Terminal isn't running");
        std::process::exit(1);
    }
    let (link, shells) = match client::connect() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("olo-terminal: can't reach olo-termd: {e}");
            std::process::exit(1);
        }
    };
    olo_ui::run(APP_ID, move |cx| {
        ui::bind_keys(cx);
        sudo::listen(olo_ui::proxy(cx));
        let proxy = olo_ui::proxy(cx);
        let look = olo_ui::theme::palette(cx);
        let state = olo_ui::restored_state();
        let term = cx.new(|cx: &mut Cx| {
            let mut t = Terminal {
                link,
                windows: HashMap::new(),
                settings: Settings::load(),
                metrics: None,
                look: look.clone(),
                search: find::Search::default(),
                respawn: false,
                front: None,
                saved_groups: groups::load(),
                agents: agent::Watcher::start(proxy.clone()),
                git: git::Watcher::start(proxy.clone()),
                worktrees: HashMap::new(),
                branches: HashMap::new(),
                prs: HashMap::new(),
                worktrees_at: olo_protocol::TerminalSettings::default().worktrees,
                notifier: Notifier::new(proxy.clone(), "Terminal", "olo-terminal"),
                notes: HashMap::new(),
                remembered: None,
                remembering: false,
                retitling: false,
                agent_context: None,
                agent_snapshot: None,
                sudo_approved: HashMap::new(),
            };
            t.follow(&look);
            sudo::WINDOW_FOR_TYPED.store(t.settings.sudo_window, std::sync::atomic::Ordering::Relaxed);
            let saved = state.as_deref().and_then(|s| serde_json::from_str::<Saved>(s).ok());
            let legacy = state.as_deref().filter(|_| saved.is_none()).and_then(|s| serde_json::from_str::<Legacy>(s).ok());
            let adopted = legacy.is_some_and(|l| t.adopt(cx, l));
            // Not an upgrade: the windows as they were when Terminal last
            // quit (or the session ended), shells that have gone restored.
            let fresh = state.is_none();
            let saved = saved.or_else(|| fresh.then(remembered).flatten());
            let reattached = t.reattach(cx, shells, saved.as_ref(), fresh) || adopted;
            // Agents' titles back from before: let go if their agents
            // aren't seen again soon.
            cx.spawn(async move |me, cx| {
                cx.background_executor().timer(AGENT_SEEN_WITHIN).await;
                let _ = me.update(cx, |t, cx| t.expire_agent_titles(cx));
            })
            .detach();
            // Asked for a new window (or there's nothing to show): open one.
            if !reattached || (state.is_none() && !args.is_empty()) {
                t.handle_launch(cx, Launch { args, cwd: std::env::current_dir().ok(), taken: None });
            }
            t
        });
        let t = term.clone();
        olo_ui::message_handler(cx, move |message, cx| t.update(cx, |t, cx| t.message(cx, message)));
        let t = term.clone();
        olo_ui::on_upgrade(cx, move |cx| t.read(cx).save_state(cx));
        let t = term.downgrade();
        widgets::strip::on_window_dropped(cx, move |window, onto, place, cx| {
            let (Some(window), Some(onto)) = (ui::window_of(cx, window), ui::window_of(cx, onto)) else { return };
            let _ = t.update(cx, |t, cx| t.window_dropped(cx, window, onto, Some(place)));
        });
        olo_kit_base::settings::watch(move |s| {
            proxy.send(AgentContext { groups: s.terminal.agent_context, titles: s.terminal.agent_titles });
            proxy.send(repos::WorktreesAt(s.terminal.worktrees));
        });
    });
}

/// `s` as one shell word: as it is when that is safe, else single-quoted.
fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"/-._+,:@%=".contains(&b)) {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod drop_tests {
    #[test]
    fn shell_quote() {
        assert_eq!(super::shell_quote("/tmp/a.png"), "/tmp/a.png");
        assert_eq!(super::shell_quote("/tmp/My Photo.png"), "'/tmp/My Photo.png'");
        assert_eq!(super::shell_quote("/tmp/it's"), "'/tmp/it'\\''s'");
    }
}

#[cfg(test)]
mod settings_tests {
    use super::{SchemeChoice, Settings};

    fn colors(s: &Settings) -> (Option<&str>, Option<&str>) {
        (s.light_colors.as_deref(), s.dark_colors.as_deref())
    }

    #[test]
    fn colours_for_each_appearance() {
        // New: the look's own, light and dark.
        let s = Settings::parse("{}");
        assert_eq!(colors(&s), (None, None));
        let old = r#"{"text_size":12.0,"cols":120,"rows":25,"scheme":"#;
        // Saved before the choice: matching the appearance takes the
        // look's, while one who picked Olo Light (or Dark) keeps it, now
        // BeOS's.
        let s = Settings::parse(&format!("{old}\"Auto\"}}"));
        assert_eq!((s.scheme, colors(&s)), (SchemeChoice::Auto, (None, None)));
        let s = Settings::parse(&format!("{old}\"Light\"}}"));
        assert_eq!((s.scheme, colors(&s)), (SchemeChoice::Light, (Some("beos-light"), None)));
        let s = Settings::parse(&format!("{old}\"Dark\"}}"));
        assert_eq!((s.scheme, colors(&s)), (SchemeChoice::Dark, (None, Some("beos-dark"))));
        // Saved since: as saved (Olo's own now BeOS's).
        let s = Settings::parse(&format!("{old}\"Light\",\"light_colors\":\"frappe\",\"dark_colors\":\"olo-dark\"}}"));
        assert_eq!(colors(&s), (Some("frappe"), Some("beos-dark")));
        let round = Settings::parse(&serde_json::to_string(&s).unwrap());
        assert_eq!((round.scheme, colors(&round)), (s.scheme, colors(&s)));
    }
}
