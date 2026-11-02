//! Repositories in Terminal (git.rs does the git): a window whose tabs
//! are all in one repository becomes a group named after it, on its own;
//! the group's menu lists the repository's worktrees (one chosen opens in
//! a new tab) and makes new ones; a tab in a linked worktree can rename
//! or remove it; and a tab on a branch with a pull request on GitHub
//! shows its state and opens it.

use std::{collections::HashMap, path::PathBuf, sync::Arc, sync::atomic::Ordering};

use olo_theme::Hue;
use olo_ui::{
    Menu,
    gpui::{AnyElement, App, Bounds, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Styled as _, Window, div, px},
    metrics as m,
    style::{self, PullRequestShape},
    widgets::{self, Button, StatusBar, StringView, TextControl, TextField},
};

use crate::{
    Tab, Terminal, Win, git,
    groups::Group,
    links,
    session::Start,
    ui::{Cx, WinOps as _, WindowId, WindowOptions},
};

/// New Worktree… or Rename Worktree…, for a window of the repository.
pub struct Dialog {
    pub target: WindowId,
    main: PathBuf,
    repo: String,
    /// Renaming this worktree, rather than making one.
    rename: Option<PathBuf>,
    /// The branch a new one starts from (the main worktree's).
    from: Option<String>,
    name: Entity<TextField>,
    error: Option<String>,
    job: Option<Arc<git::Progress>>,
}

impl Dialog {
    /// Closed while making one: it undoes itself, after the batch of
    /// files it's on.
    pub fn stop(&self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// A worktree made or renamed, for the dialog `.0`.
pub struct Done(WindowId, Result<PathBuf, String>);
/// Remove Worktree… looked at what is in it: ask.
pub struct Ask {
    main: PathBuf,
    path: PathBuf,
    changes: usize,
}
/// A worktree removed (or not).
pub struct Removed(PathBuf, Result<(), String>);

const DIALOG_W: f32 = 400.0;
const NOTE_H: f32 = 2.0 * m::LABEL_ROW;
const STATUS_H: f32 = m::LABEL_ROW + m::TIGHT + 14.0;
const DIALOG_H: f32 = 2.0 * m::WINDOW_INSET + m::CONTROL_HEIGHT + m::SPACING + NOTE_H + m::SPACING + STATUS_H + m::SPACING + m::CONTROL_HEIGHT;

/// A tab in a repository on GitHub, with no sign-in to see its pull
/// requests (private ones answer nothing without one).
fn needs_github(tab: &Tab) -> bool {
    tab.repo.as_ref().is_some_and(|r| !r.github.is_empty()) && git::signed_out()
}

/// The pull request for the branch `tab` is on, if GitHub has one.
pub fn pr_of<'a>(prs: &'a HashMap<git::PrKey, git::Pr>, tab: &Tab) -> Option<&'a git::Pr> {
    let r = tab.repo.as_ref()?;
    let branch = r.branch.as_ref()?;
    let head = &r.github.first()?.0;
    r.github.iter().find_map(|(owner, repo)| prs.get(&git::PrKey { owner: owner.clone(), repo: repo.clone(), head: head.clone(), branch: branch.clone() }))
}

/// A pull request's mark for its tab: GitHub's, in its state's colour,
/// its commit filled with its checks' (green passing, red failing, orange
/// running).
pub fn pr_badge(pr: &git::Pr) -> olo_ui::widgets::strip::PaintFn {
    let (state, checks) = (pr.state, pr.checks);
    std::rc::Rc::new(move |window: &mut Window, cx: &mut App, r: Bounds<Pixels>| {
        use git::{Checks, PrState};
        let p = olo_ui::theme::palette(cx);
        let (shape, color) = match state {
            PrState::Open => (PullRequestShape::Open, style::c(p.success)),
            PrState::Draft => (PullRequestShape::Draft, style::disabled_text(&p)),
            PrState::Merged => (PullRequestShape::Merged, style::c(p.hue(Hue::Purple))),
            PrState::Closed => (PullRequestShape::Closed, style::c(p.failure)),
        };
        let node = checks.map(|c| match c {
            Checks::Passing => style::c(p.success),
            Checks::Failing => style::c(p.failure),
            Checks::Pending => style::c(p.hue(Hue::Orange)),
        });
        style::pull_request_mark(window, r, shape, color, node);
    })
}

impl Terminal {
    /// What git.rs's watcher saw.
    pub(crate) fn git_report(&mut self, cx: &mut Cx, report: git::Report) {
        match report {
            git::Report::Repo(shell, repo) => {
                let Some((win, i)) = self.find_shell(shell) else { return };
                let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
                t.tabs[i].repo = repo;
                cx.redraw(win);
                self.auto_group(cx, win);
            }
            git::Report::Worktrees(main, list, names) => {
                self.worktrees.insert(main.clone(), list);
                self.branches.insert(main, names);
            }
            git::Report::Pr(key, pr) => {
                let changed = match pr {
                    Some(pr) => self.prs.insert(key, pr.clone()).as_ref() != Some(&pr),
                    None => self.prs.remove(&key).is_some(),
                };
                if changed {
                    cx.redraw_all();
                }
            }
        }
    }

    /// Each tab of `win` in a repository is in that repository's group,
    /// as a browser's tabs of a site: the group beside it, else one made
    /// for it, in the colour that repository's group has in another window
    /// (if one has). Not a tab the person took out of a group, nor one
    /// whose repository's group is elsewhere in the window (a tab doesn't
    /// jump there on its own). A tab no longer in the repository of the
    /// group it joined on its own leaves it; a group left empty goes.
    pub(crate) fn auto_group(&mut self, cx: &mut Cx, win: WindowId) {
        let colors: Vec<(PathBuf, usize)> = self
            .windows
            .iter()
            .filter(|(id, _)| **id != win)
            .filter_map(|(_, w)| if let Win::Term(x) = w { Some(x) } else { None })
            .flat_map(|x| x.groups.iter().filter_map(|g| Some((g.repo.clone()?, g.color))))
            .collect();
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return };
        let mut changed = false;
        for i in 0..t.tabs.len() {
            let main = t.tabs[i].repo.as_ref().map(|r| r.main.clone());
            if let Some(g) = t.tabs[i].group
                && t.groups[g].auto
                && t.groups[g].repo != main
            {
                t.tabs[i].group = None;
                changed = true;
            }
        }
        for i in 0..t.tabs.len() {
            if t.tabs[i].group.is_some() || t.tabs[i].own_grouping {
                continue;
            }
            let Some(repo) = t.tabs[i].repo.as_ref() else { continue };
            let (name, main) = (repo.name.clone(), repo.main.clone());
            let beside = |j: Option<usize>| j.and_then(|j| t.tabs.get(j)).and_then(|x| x.group).filter(|&g| t.groups[g].repo.as_ref() == Some(&main));
            let g = match beside(i.checked_sub(1)).or(beside(Some(i + 1))) {
                Some(g) => g,
                None if t.groups.iter().any(|g| g.repo.as_ref() == Some(&main)) => continue,
                None => {
                    let used: Vec<usize> = t.groups.iter().map(|g| g.color).collect();
                    let color = colors.iter().find(|(m, _)| *m == main).map_or_else(|| crate::groups::new_color(&used), |(_, c)| *c);
                    t.groups.push(Group { name, color, repo: Some(main), auto: true });
                    t.groups.len() - 1
                }
            };
            t.tabs[i].group = Some(g);
            changed = true;
        }
        if changed {
            t.tidy();
            cx.redraw(win);
        }
    }

    /// The repository `win` is for: its group's, else its tab's.
    pub(crate) fn repo_of(&self, win: WindowId) -> Option<(PathBuf, String)> {
        let Some(Win::Term(t)) = self.windows.get(&win) else { return None };
        let named = |main: &PathBuf| main.file_name().map(|n| n.to_string_lossy().trim_end_matches(".git").to_owned()).unwrap_or_default();
        if let Some(main) = t.group().and_then(|g| g.repo.clone()) {
            let name = t.tabs.iter().filter_map(|x| x.repo.as_ref()).find(|r| r.main == main).map_or_else(|| named(&main), |r| r.name.clone());
            return Some((main, name));
        }
        t.tab().repo.as_ref().map(|r| (r.main.clone(), r.name.clone()))
    }

    /// The group menu's part for its repository: each worktree (the one
    /// the tab showing is in checked), then New Worktree….
    pub(crate) fn worktrees_menu(&self, win: WindowId, mut menu: Menu) -> Menu {
        let Some((main, _)) = self.repo_of(win) else { return menu };
        let Some(Win::Term(t)) = self.windows.get(&win) else { return menu };
        let here = t.tab().repo.as_ref().map(|r| r.here.clone());
        menu = menu.separator();
        for w in self.worktrees.get(&main).into_iter().flatten() {
            let label = match &w.branch {
                Some(b) if *b != w.name => format!("{} — {b}", w.name),
                _ => w.name.clone(),
            };
            menu = menu.item(label, format!("worktree-open:{}", w.path.display())).checked(here.as_ref() == Some(&w.path));
        }
        menu.item("New Worktree…", "worktree-new")
    }

    /// The tab menu's part for the tab running `shell`: its pull request,
    /// and its worktree if it's a linked one.
    pub(crate) fn tab_repo_menu(&self, tab: &Tab, mut menu: Menu) -> Menu {
        let shell = tab.session.id;
        let pr = pr_of(&self.prs, tab);
        let linked = tab.repo.as_ref().is_some_and(|r| r.linked);
        if pr.is_some() || linked {
            menu = menu.separator();
        }
        if let Some(pr) = pr {
            menu = menu.item(format!("Open Pull Request #{}", pr.number), format!("pr-open:{shell}"));
        } else if needs_github(tab) {
            menu = menu.separator().item("Connect GitHub for Pull Requests…", "github-connect");
        }
        if linked {
            menu = menu.item("Rename Worktree…", format!("worktree-rename:{shell}")).item("Remove Worktree…", format!("worktree-remove:{shell}"));
        }
        menu
    }

    /// Tabs ▸ Repository: the group menu's worktrees and the tab menu's
    /// items for the tab showing, from the menu bar (for agents too).
    pub(crate) fn repository_menu(&self, win: WindowId) -> Menu {
        let title = "Repository";
        let (Some(t), Some(_)) = (self.term(win), self.repo_of(win)) else {
            return Menu::new(title).item("Not in a Repository", "none").enabled(false);
        };
        let tab = t.tab();
        let shell = tab.session.id;
        // (The worktrees, after the separator the group menu wants.)
        let mut menu = Menu::new(title);
        for item in self.worktrees_menu(win, Menu::new(title)).items.into_iter().skip(1) {
            menu.items.push(item);
        }
        let pr = pr_of(&self.prs, tab);
        let linked = tab.repo.as_ref().is_some_and(|r| r.linked);
        menu = menu
            .separator()
            .item("Rename Worktree…", format!("worktree-rename:{shell}"))
            .enabled(linked)
            .item("Remove Worktree…", format!("worktree-remove:{shell}"))
            .enabled(linked)
            .separator();
        match pr {
            Some(pr) => menu.item(format!("Open Pull Request #{}", pr.number), format!("pr-open:{shell}")),
            None if needs_github(tab) => menu.item("Connect GitHub for Pull Requests…", "github-connect"),
            None => menu.item("No Pull Request", "none").enabled(false),
        }
    }

    /// The repository commands, if `command` is one.
    pub(crate) fn repo_command(&mut self, cx: &mut Cx, window: WindowId, command: &str) -> bool {
        let tab_of = |this: &Terminal, n: &str| -> Option<(WindowId, usize)> { n.parse().ok().and_then(|s| this.find_shell(s)) };
        if command == "worktree-new" {
            self.worktree_dialog(cx, window, None);
        } else if let Some(path) = command.strip_prefix("worktree-open:") {
            self.add_tab(cx, window, Start::Spawn { command: None, cwd: Some(PathBuf::from(path)) });
            cx.activate(window);
        } else if let Some((win, i)) = command.strip_prefix("worktree-rename:").and_then(|n| tab_of(self, n)) {
            let here = self.term(win).and_then(|t| t.tabs[i].repo.as_ref()).filter(|r| r.linked).map(|r| r.here.clone());
            if let Some(here) = here {
                self.worktree_dialog(cx, win, Some(here));
            }
        } else if let Some((win, i)) = command.strip_prefix("worktree-remove:").and_then(|n| tab_of(self, n)) {
            let Some(r) = self.term(win).and_then(|t| t.tabs[i].repo.clone()).filter(|r| r.linked) else { return true };
            // Counting its changes walks the worktree: not here.
            let proxy = cx.proxy();
            std::thread::spawn(move || {
                let changes = git::changes(&r.here);
                proxy.send(Ask { main: r.main, path: r.here, changes });
            });
        } else if command == "github-connect" {
            // Where every service is connected.
            if let Err(e) = olo_xdg::spawn_detached("olo-preferences integrations") {
                tracing::warn!("couldn't open Preferences: {e}");
            }
        } else if let Some((win, i)) = command.strip_prefix("pr-open:").and_then(|n| tab_of(self, n)) {
            if let Some(pr) = self.term(win).and_then(|t| pr_of(&self.prs, &t.tabs[i])) {
                links::open(&links::Target::Url(pr.url.clone()));
            }
        } else {
            return false;
        }
        true
    }

    fn term(&self, win: WindowId) -> Option<&crate::TermWin> {
        match self.windows.get(&win) {
            Some(Win::Term(t)) => Some(t),
            _ => None,
        }
    }

    /// New Worktree… (`rename` None) or Rename Worktree… for `win`'s
    /// repository.
    fn worktree_dialog(&mut self, cx: &mut Cx, win: WindowId, rename: Option<PathBuf>) {
        let Some((main, repo)) = self.repo_of(win) else { return };
        let open = self.windows.iter().find(|(_, w)| matches!(w, Win::Worktree(d) if d.target == win));
        if let Some((&id, _)) = open {
            return cx.activate(id);
        }
        let name = rename.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let name = Terminal::field(cx, &name, |f| f.hint("fix-tabs").font(style::mono_font(FontWeight::NORMAL)));
        let from = self.worktrees.get(&main).and_then(|l| l.iter().find(|w| w.main)).and_then(|w| w.branch.clone());
        let title = if rename.is_some() { "Rename Worktree" } else { "New Worktree" };
        let (w, h) = (DIALOG_W as u32, DIALOG_H as u32);
        let id = cx.open_window(WindowOptions::new(title, w, h).min_size(w, h).max_size(w, h));
        cx.subscribe(&name, move |t: &mut Terminal, _, e: &widgets::TextFieldEvent, cx| match e {
            widgets::TextFieldEvent::Entered => t.worktree_submit(cx, id),
            widgets::TextFieldEvent::Changed => {
                if let Some(Win::Worktree(d)) = t.windows.get_mut(&id) {
                    d.error = None;
                }
                cx.redraw(id);
            }
            widgets::TextFieldEvent::Typed(_) => {}
        })
        .detach();
        let f = name.clone();
        cx.with_window(id, move |_, window, cx| f.read(cx).focus(window));
        self.windows.insert(id, Win::Worktree(Box::new(Dialog { target: win, main, repo, rename, from, name, error: None, job: None })));
    }

    /// What the worktree dialog says of the name typed: where it goes,
    /// and why it can't be, if it can't.
    fn worktree_note(&self, d: &Dialog, name: &str) -> (String, Option<String>) {
        let place = self.worktree_place(d, name);
        let shown = tilde(&place);
        let branches = self.branches.get(&d.main).map(Vec::as_slice).unwrap_or_default();
        let trees = self.worktrees.get(&d.main).map(Vec::as_slice).unwrap_or_default();
        let from = d.from.as_deref().map(|f| format!(" from {f}")).unwrap_or_default();
        let mut invalid = (!name.is_empty()).then(|| git::valid_name(name).err()).flatten();
        let note = match &d.rename {
            Some(p) => {
                let old = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let branch = trees.iter().find(|w| w.path == *p).and_then(|w| w.branch.clone());
                match branch {
                    Some(b) if b == old && !branches.iter().any(|b| b == name) => format!("Its folder becomes {shown}, and its branch, {old}, takes the new name too."),
                    _ => format!("Its folder becomes {shown}; its branch stays as it is."),
                }
            }
            None if name.is_empty() => format!("In {shown}, on a new branch of its name{from}."),
            None if branches.iter().any(|b| b == name) => {
                if let Some(w) = trees.iter().find(|w| w.branch.as_deref() == Some(name)) {
                    invalid = invalid.or_else(|| Some(format!("Branch {name} is checked out in {} already.", tilde(&w.path))));
                }
                format!("In {shown}, on its branch, {name}.")
            }
            None => match &d.from {
                Some(f) => format!("In {shown}, on a new branch, {name}, from {f}."),
                None => format!("In {shown}, on a new branch, {name}."),
            },
        };
        (note, invalid)
    }

    fn worktree_place(&self, d: &Dialog, name: &str) -> PathBuf {
        let shown = if name.is_empty() { "…" } else { name };
        match &d.rename {
            Some(p) => p.with_file_name(shown),
            None => git::place(&self.worktrees_at, &d.repo, shown),
        }
    }

    /// New Worktree… or Rename Worktree…: the name, where it goes, how
    /// it's going.
    pub(crate) fn render_worktree(&mut self, id: WindowId, focus: FocusHandle, _: &mut Window, cx: &mut Cx) -> AnyElement {
        let Some(Win::Worktree(d)) = self.windows.get(&id) else { return div().into_any_element() };
        let busy = d.job.is_some();
        let name = d.name.read(cx).text().trim().to_owned();
        let (note, invalid) = self.worktree_note(d, &name);
        let ready = !busy && !name.is_empty() && invalid.is_none() && d.rename.as_ref().is_none_or(|p| p.file_name().is_none_or(|n| n.to_string_lossy() != name));
        d.name.update(cx, |f, cx| f.set_enabled(!busy, cx));
        let p = olo_ui::theme::palette(cx);
        let status = if let Some(job) = &d.job {
            let (done, total) = (job.done.load(Ordering::Relaxed), job.total.load(Ordering::Relaxed));
            let text = if total == 0 { "Making it…".to_owned() } else { format!("Checking out {done} of {total} files") };
            let f = if total == 0 { 0.0 } else { done as f32 / total as f32 };
            // (Its progress, drawn again while it goes.)
            cx.spawn(async move |me, cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
                let _ = me.update(cx, |_, cx| cx.redraw(id));
            })
            .detach();
            StatusBar::new(f).label(text).into_any_element()
        } else if let Some(e) = d.error.clone().or(invalid) {
            StringView::new(e).color(style::c(p.failure)).wrap(true).into_any_element()
        } else {
            div().into_any_element()
        };
        let label = if d.rename.is_some() { "Rename" } else { "Create" };
        let field = d.name.clone();
        let cancel = cx.listener(move |t, _: &(), _, cx| t.worktree_cancel(cx, id));
        let ok = cx.listener(move |t, _: &(), _, cx| t.worktree_submit(cx, id));
        let row = widgets::button_row()
            .child(Button::new(if busy { "Stop" } else { "Cancel" }).on_click(move |window, cx| cancel(&(), window, cx)))
            .child(Button::new(label).default(true).enabled(ready).on_click(move |window, cx| ok(&(), window, cx)));
        div()
            .size_full()
            .key_context(widgets::alert::CONTEXT)
            .track_focus(&focus)
            .on_action(cx.listener(move |t, _: &widgets::alert::Cancel, _, cx| t.worktree_cancel(cx, id)))
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .child(TextControl::new(field).label("Name:").divider(50.0))
            .child(div().h(px(m::SPACING)))
            .child(div().h(px(NOTE_H)).child(StringView::new(note).dim(true).wrap(true)))
            .child(div().h(px(m::SPACING)))
            .child(div().h(px(STATUS_H)).child(status))
            .child(div().flex_1())
            .child(row)
            .into_any_element()
    }

    fn worktree_cancel(&mut self, cx: &mut Cx, window: WindowId) {
        if let Some(Win::Worktree(d)) = self.windows.get(&window) {
            d.stop();
        }
        self.windows.remove(&window);
        cx.close_window(window);
    }

    /// Create (or Rename): on a thread.
    fn worktree_submit(&mut self, cx: &mut Cx, window: WindowId) {
        let Some(Win::Worktree(d)) = self.windows.get(&window) else { return };
        let name = d.name.read(cx).text().trim().to_owned();
        let (_, invalid) = self.worktree_note(d, &name);
        if d.job.is_some() || name.is_empty() || invalid.is_some() {
            return;
        }
        let place = self.worktree_place(d, &name);
        let proxy = cx.proxy();
        let Some(Win::Worktree(d)) = self.windows.get_mut(&window) else { return };
        let main = d.main.clone();
        match d.rename.clone() {
            Some(path) => {
                d.job = Some(Arc::default());
                std::thread::spawn(move || proxy.send(Done(window, git::rename(&main, &path, &name))));
            }
            None => {
                let job: Arc<git::Progress> = Arc::default();
                d.job = Some(job.clone());
                std::thread::spawn(move || proxy.send(Done(window, git::add(&main, &name, &place, &job))));
            }
        }
        cx.redraw(window);
    }

    /// A worktree made (open it in a tab of the window it was asked from)
    /// or renamed; or why not, in its dialog.
    pub(crate) fn worktree_done(&mut self, cx: &mut Cx, Done(dialog, result): Done) {
        let Some(Win::Worktree(d)) = self.windows.get_mut(&dialog) else { return };
        d.job = None;
        match result {
            Err(e) => {
                d.error = Some(e);
                cx.redraw(dialog);
            }
            Ok(path) => {
                let (target, made) = (d.target, d.rename.is_none());
                self.windows.remove(&dialog);
                cx.close_window(dialog);
                if made && self.windows.contains_key(&target) {
                    self.add_tab(cx, target, Start::Spawn { command: None, cwd: Some(path) });
                    cx.activate(target);
                }
            }
        }
    }

    /// Remove Worktree…: what goes, and what stays.
    pub(crate) fn worktree_ask(&mut self, cx: &mut Cx, a: Ask) {
        let name = a.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let inside = self.tabs_in(&a.path);
        let running: Vec<String> = inside
            .iter()
            .filter_map(|&(win, shell)| self.term(win)?.tabs.iter().find(|x| x.session.id == shell))
            .filter(|x| x.session.busy())
            .filter_map(|x| x.session.foreground())
            .collect();
        let tabs = inside.len();
        let changes = match a.changes {
            0 => String::new(),
            1 => ", with the 1 file changed in it and not committed".into(),
            n => format!(", with the {n} files changed in it and not committed"),
        };
        let tabs = match tabs {
            0 => String::new(),
            1 => " Its tab closes.".into(),
            n => format!(" Its {n} tabs close."),
        };
        let running = match running.as_slice() {
            [] => String::new(),
            [one] => format!(" “{one}” is still running in it, and stops."),
            [first, rest @ ..] => format!(" “{first}” and {} more are still running in them, and stop.", rest.len()),
        };
        let message = format!("Remove the worktree “{name}”?\n\nIts folder, {}, goes{changes}. Its branch stays.{tabs}{running}", tilde(&a.path));
        let id = cx.open_window(WindowOptions::alert("Terminal", &message));
        self.windows.insert(id, Win::ConfirmRemove { main: a.main, path: a.path, message });
    }

    /// The tabs in the worktree at `path`.
    fn tabs_in(&self, path: &PathBuf) -> Vec<(WindowId, u64)> {
        let mut out = Vec::new();
        for (&id, w) in &self.windows {
            if let Win::Term(t) = w {
                out.extend(t.tabs.iter().filter(|x| x.repo.as_ref().is_some_and(|r| &r.here == path)).map(|x| (id, x.session.id)));
            }
        }
        out
    }

    /// Remove it, after its tabs close (so nothing goes on writing in it).
    pub(crate) fn remove_worktree(&mut self, cx: &mut Cx, main: PathBuf, path: PathBuf) {
        for (win, shell) in self.tabs_in(&path) {
            self.close_tab(cx, win, shell);
        }
        let repo = main.file_name().map(|n| n.to_string_lossy().trim_end_matches(".git").to_owned()).unwrap_or_default();
        let tidy = path.parent() == git::place(&self.worktrees_at, &repo, "x").parent();
        let proxy = cx.proxy();
        std::thread::spawn(move || {
            let result = git::remove(&main, &path, tidy);
            proxy.send(Removed(path, result));
        });
    }

    pub(crate) fn worktree_removed(&mut self, cx: &mut Cx, Removed(path, result): Removed) {
        if let Err(e) = result {
            let message = format!("Couldn’t remove the worktree in {}: {e}", tilde(&path));
            let id = cx.open_window(WindowOptions::alert("Terminal", &message));
            self.windows.insert(id, Win::Failed { message });
        }
    }

    /// What the repository's threads sent: None if `message` was theirs,
    /// else it, for whoever it's for.
    pub(crate) fn repo_message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) -> Option<Box<dyn std::any::Any + Send>> {
        let message = match message.downcast::<git::Report>() {
            Ok(r) => {
                self.git_report(cx, *r);
                return None;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<Done>() {
            Ok(d) => {
                self.worktree_done(cx, *d);
                return None;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<Ask>() {
            Ok(a) => {
                self.worktree_ask(cx, *a);
                return None;
            }
            Err(m) => m,
        };
        let message = match message.downcast::<Removed>() {
            Ok(r) => {
                self.worktree_removed(cx, *r);
                return None;
            }
            Err(m) => m,
        };
        match message.downcast::<WorktreesAt>() {
            Ok(at) => {
                self.worktrees_at = at.0;
                cx.redraw_all();
                None
            }
            Err(m) => Some(m),
        }
    }
}

/// Preferences ▸ Terminal's worktrees location, as it changes.
pub struct WorktreesAt(pub String);

/// `p` with the home folder as `~`.
fn tilde(p: &std::path::Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home.as_ref().and_then(|h| p.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}
