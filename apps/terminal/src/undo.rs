//! Undo and Redo in Terminal (Ben, 2026-11-07: "we need generally and
//! unviversally to support undo in all olo apps where it makes sense"), on
//! the kit's way (`olo_kit::undo`): a window's tabs moved or put in and out
//! of groups (their order and groups put back), a tab closed, and
//! Terminal's settings (text size, colours, where the tabs are). A closed
//! tab's process can't come back: Undo opens a new shell in its folder,
//! where it was, in its group, and says so (Undo Close Tab (as a New
//! Shell)); Redo closes that one again, asking first as any close does if
//! something runs in it. Edit ▸ Undo and Redo, with Terminal's own keys
//! in the profile (`terminal.undo`, `terminal.redo`: they never reach the
//! shell).

use std::path::PathBuf;

use olo_kit::undo::{History, Purpose};

use crate::{
    Group, Start, Terminal, Win,
    ui::{Cx, WindowId},
};

/// What a step holds to take it back.
#[derive(Debug, Clone)]
pub(crate) enum TabStep {
    /// A window's tabs in this order (each by its shell), in these groups.
    Order {
        window: WindowId,
        tabs: Vec<(u64, Option<usize>, bool)>,
        groups: Vec<Group>,
    },
    /// A tab closed: where it was, its folder, its group.
    Closed {
        window: WindowId,
        at: usize,
        cwd: Option<PathBuf>,
        group: Option<Group>,
    },
    /// The tab Undo opened in a closed one's place (Redo closes it).
    Reopened {
        window: WindowId,
        shell: u64,
    },
    /// A tab moved into a window of its own (torn off, Move to New
    /// Window): the window, place and group it came from.
    Detached {
        shell: u64,
        from: WindowId,
        at: usize,
        group: Option<Group>,
    },
    /// A tab put back in `window` by Undo (Redo moves it out again).
    Rejoined {
        shell: u64,
        window: WindowId,
    },
    Settings(Kept),
}

/// The settings a step puts back.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Kept {
    text_size: f32,
    scheme: crate::SchemeChoice,
    light_colors: Option<String>,
    dark_colors: Option<String>,
    side_tabs: bool,
    sudo_window: bool,
}

pub(crate) type Steps = History<TabStep>;

/// What a closed tab's step is called: it comes back as a new shell.
const CLOSED: &str = "Close Tab (as a New Shell)";

impl Terminal {
    /// Window `win`'s tabs and groups as they are now.
    pub(crate) fn tab_order(&self, win: WindowId) -> Option<TabStep> {
        let Some(Win::Term(t)) = self.windows.get(&win) else { return None };
        Some(TabStep::Order { window: win, tabs: t.tabs.iter().map(|x| (x.session.id, x.group, x.own_grouping)).collect(), groups: t.groups.clone() })
    }

    /// After something moved `win`'s tabs: a step, if they did move.
    pub(crate) fn tabs_moved(&mut self, win: WindowId, before: Option<TabStep>, name: &str) {
        let (Some(before), Some(now)) = (before, self.tab_order(win)) else { return };
        if let (TabStep::Order { tabs: a, .. }, TabStep::Order { tabs: b, .. }) = (&before, &now)
            && a != b
        {
            self.history.done(name, before);
        }
    }

    /// Tab `shell` of `win` about to close: what Undo needs to bring a new
    /// one there.
    pub(crate) fn closing(&self, win: WindowId, shell: u64) -> Option<TabStep> {
        let Some(Win::Term(t)) = self.windows.get(&win) else { return None };
        let at = t.position(shell)?;
        let tab = &t.tabs[at];
        Some(TabStep::Closed { window: win, at, cwd: tab.session.cwd(), group: tab.group.and_then(|g| t.groups.get(g)).cloned() })
    }

    /// A tab closed (`step`, from `closing`): a step, done or undone or
    /// redone as its close was asked for.
    pub(crate) fn closed(&mut self, shell: u64, step: TabStep) {
        let purpose = self.closing_as.remove(&shell).unwrap_or(Purpose::Do);
        self.record(purpose, CLOSED, step);
    }

    fn record(&mut self, purpose: Purpose, name: &str, step: TabStep) {
        match purpose {
            Purpose::Do => self.history.done(name, step),
            p => self.history.record(p, name, step),
        }
    }

    pub(crate) fn kept(&self) -> Kept {
        let s = &self.settings;
        Kept { text_size: s.text_size, scheme: s.scheme, light_colors: s.light_colors.clone(), dark_colors: s.dark_colors.clone(), side_tabs: s.side_tabs, sudo_window: s.sudo_window }
    }

    /// After a settings command: a step if it changed them (text sizes
    /// one after another, one step).
    pub(crate) fn settings_changed(&mut self, before: Kept, name: &str) {
        if before == self.kept() {
            return;
        }
        if before.text_size != self.settings.text_size && name == "Text Size" {
            self.history.done_merging("text-size", name, TabStep::Settings(before), |_, _| {});
        } else {
            self.history.done(name, TabStep::Settings(before));
        }
    }

    fn put_settings(&mut self, cx: &mut Cx, k: Kept) {
        if k.text_size != self.settings.text_size {
            self.set_text_size(cx, k.text_size);
        }
        let s = &mut self.settings;
        s.scheme = k.scheme;
        s.light_colors = k.light_colors;
        s.dark_colors = k.dark_colors;
        s.side_tabs = k.side_tabs;
        s.sudo_window = k.sudo_window;
        crate::sudo::WINDOW_FOR_TYPED.store(s.sudo_window, std::sync::atomic::Ordering::Relaxed);
        s.save();
        cx.redraw_all();
    }

    /// Tab `shell` of `win` about to get a window of its own: where it
    /// was, for Undo.
    pub(crate) fn detaching(&self, win: WindowId, shell: u64) -> Option<TabStep> {
        let Some(Win::Term(t)) = self.windows.get(&win) else { return None };
        let at = t.position(shell)?;
        Some(TabStep::Detached { shell, from: win, at, group: t.tabs[at].group.and_then(|g| t.groups.get(g)).cloned() })
    }

    /// It did (`step` from `detaching`): a step, done or redone.
    pub(crate) fn detached(&mut self, step: TabStep) {
        let purpose = self.detaching_as.take().unwrap_or(Purpose::Do);
        self.record(purpose, "Move to New Window", step);
    }

    /// Edit ▸ Undo (`redo` false) or Redo.
    pub(crate) fn undo_redo(&mut self, cx: &mut Cx, redo: bool) {
        let Some(step) = (if redo { self.history.take_redo() } else { self.history.take_undo() }) else { return };
        let purpose = if redo { Purpose::Redo } else { Purpose::Undo };
        match step.change {
            TabStep::Order { window, tabs, groups } => {
                let now = self.tab_order(window);
                let Some(Win::Term(t)) = self.windows.get_mut(&window) else { return };
                let shown = t.tab().session.id;
                let mut rest: Vec<Option<crate::Tab>> = std::mem::take(&mut t.tabs).into_iter().map(Some).collect();
                let mut order = Vec::with_capacity(rest.len());
                for (id, group, own) in &tabs {
                    if let Some(i) = rest.iter().position(|x| x.as_ref().is_some_and(|x| x.session.id == *id))
                        && let Some(mut tab) = rest[i].take()
                    {
                        tab.group = group.filter(|g| *g < groups.len());
                        tab.own_grouping = *own;
                        order.push(tab);
                    }
                }
                // (Tabs made since, at the end, in no group.)
                for mut tab in rest.into_iter().flatten() {
                    tab.group = None;
                    order.push(tab);
                }
                t.tabs = order;
                t.groups = groups;
                t.active = t.position(shown).unwrap_or(0);
                t.tidy();
                cx.redraw(window);
                if let Some(now) = now {
                    self.record(purpose, &step.name, now);
                }
            }
            TabStep::Closed { window, at, cwd, group } => {
                // Its window gone: a window of its own.
                if !matches!(self.windows.get(&window), Some(Win::Term(_))) {
                    self.open(cx, cwd, None);
                    return;
                }
                let into = group.as_ref().map(|g| {
                    let Some(Win::Term(t)) = self.windows.get_mut(&window) else { return 0 };
                    match t.groups.iter().position(|x| x.name == g.name) {
                        Some(i) => i,
                        None => {
                            t.groups.push(g.clone());
                            t.groups.len() - 1
                        }
                    }
                });
                let Some(shell) = self.reopen_tab(cx, window, at, cwd, into) else { return };
                self.record(purpose, "Close Tab", TabStep::Reopened { window, shell });
            }
            TabStep::Reopened { window, shell } => {
                // (Closed as any tab is: asked first if something runs.)
                self.closing_as.insert(shell, purpose);
                self.close_tab_requested(cx, window, shell, if redo { "Edit ▸ Redo Close Tab" } else { "Edit ▸ Undo" });
            }
            TabStep::Detached { shell, from, at, group } => {
                if self.tab_back(cx, shell, from, at, group) {
                    self.record(purpose, &step.name, TabStep::Rejoined { shell, window: from });
                }
            }
            TabStep::Rejoined { shell, window } => {
                self.detaching_as = Some(purpose);
                if self.detach(cx, window, shell, false).is_none() {
                    self.detaching_as = None;
                }
            }
            TabStep::Settings(k) => {
                let now = self.kept();
                self.put_settings(cx, k);
                self.record(purpose, &step.name, TabStep::Settings(now));
            }
        }
    }

    /// Tab `shell` (wherever it is now) back into window `to`, as its tab
    /// `at`, in `group` (by name; made again if gone). Its window goes if
    /// it was its last tab. False if it or `to` is gone.
    fn tab_back(&mut self, cx: &mut Cx, shell: u64, to: WindowId, at: usize, group: Option<Group>) -> bool {
        let Some((now, i)) = self.find_shell(shell) else { return false };
        if now == to || !matches!(self.windows.get(&to), Some(Win::Term(_))) {
            return false;
        }
        let Some(Win::Term(t)) = self.windows.get_mut(&now) else { return false };
        let mut tab = if t.tabs.len() == 1 {
            let Some(Win::Term(mut t)) = self.windows.remove(&now) else { return false };
            cx.close_window(now);
            t.tabs.remove(0)
        } else {
            let tab = t.take(i);
            t.tidy();
            cx.redraw(now);
            tab
        };
        let Some(Win::Term(t)) = self.windows.get_mut(&to) else { return false };
        tab.group = group.map(|g| match t.groups.iter().position(|x| x.name == g.name) {
            Some(i) => i,
            None => {
                t.groups.push(g);
                t.groups.len() - 1
            }
        });
        tab.rang = false;
        let at = at.min(t.tabs.len());
        t.tabs.insert(at, tab);
        t.select(at);
        t.tidy();
        cx.redraw(to);
        cx.activate(to);
        true
    }

    /// A new shell in `cwd` as tab `at` of `win` (in group `into`): its id.
    fn reopen_tab(&mut self, cx: &mut Cx, win: WindowId, at: usize, cwd: Option<PathBuf>, into: Option<usize>) -> Option<u64> {
        let Some(Win::Term(t)) = self.windows.get(&win) else { return None };
        let (cols, rows) = t.grid();
        let mut tab = self.start(cx, crate::Size { cols, rows }, Start::Spawn { command: None, cwd });
        let shell = tab.session.id;
        let Some(Win::Term(t)) = self.windows.get_mut(&win) else { return None };
        tab.group = into;
        let at = at.min(t.tabs.len());
        t.tabs.insert(at, tab);
        t.select(at);
        t.tidy();
        cx.redraw(win);
        Some(shell)
    }
}
