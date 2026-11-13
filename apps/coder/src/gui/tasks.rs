//! Where the work stands, beside the conversation: the agent's latest
//! report (done, doing now, up next) on a card, and the task list as an
//! outline the person edits as freely as the agent does (a click on a
//! task's mark moves it on, its menu does the rest, a field adds one).
//! Each edit is one `edit_todos` command, applied to the list as it is
//! then; the agent is told what changed at its next step.
use super::{Coder, Msg};
use crate::form::{Field, Submission};
use olo_coder_core::{StatusReport, Todo};
use olo_kit::{
    Bounds, Cx, Element, FontWeight, IntoElement as _, Menu, ParentElement as _, Pixels, Point, Styled as _, canvas, div, menu, metrics as m, point, px, size, style, theme,
    widgets::{Column, ColumnListView, StatusBar, StringView},
};
use olo_theme::Palette;
use serde_json::json;
use std::{cell::RefCell, collections::HashMap, rc::Rc};

/// How far each level of subtasks is set in.
const INDENT: f32 = 14.0;
/// A task's mark (and the report's), points square.
const MARK: f32 = 12.0;

/// A task as the outline shows it.
#[derive(Clone)]
pub struct Row {
    pub id: String,
    pub parent: Option<String>,
    pub text: String,
    pub status: String,
    /// What shows it's done (the agent's).
    pub evidence: String,
    pub depth: usize,
    /// Whether it has subtasks, and whether they show.
    pub children: Option<bool>,
}

/// Where a row's latch and mark were drawn (for a press on them).
#[derive(Default)]
pub struct Hits {
    latch: HashMap<usize, Bounds<Pixels>>,
    mark: HashMap<usize, Bounds<Pixels>>,
}

/// The rows showing: every task, less the subtasks of folded ones.
pub fn rows(todos: &[Todo], folded: &std::collections::HashSet<String>) -> Vec<Row> {
    let mut out = Vec::with_capacity(todos.len());
    let mut hidden_under: Option<usize> = None;
    let mut depths: HashMap<&str, usize> = HashMap::new();
    for (i, t) in todos.iter().enumerate() {
        let depth = t.parent.as_deref().and_then(|p| depths.get(p)).map_or(0, |d| d + 1);
        depths.insert(&t.id, depth);
        if let Some(d) = hidden_under {
            if depth > d {
                continue;
            }
            hidden_under = None;
        }
        let has = todos.get(i + 1).is_some_and(|n| n.parent.as_deref() == Some(t.id.as_str()));
        let open = !folded.contains(&t.id);
        if has && !open {
            hidden_under = Some(depth);
        }
        out.push(Row { id: t.id.clone(), parent: t.parent.clone(), text: t.text.clone(), status: t.status.clone(), evidence: t.evidence.clone(), depth, children: has.then_some(open) });
    }
    out
}

/// A task's mark: a ring to do, half filled while in progress, a
/// filled disc with a check when done, a red disc with a bar when
/// blocked.
pub fn mark(g: &mut olo_kit::Painter, r: Bounds<Pixels>, status: &str, p: &Palette) {
    let c = style::snap_centred(g, r.center(), MARK, MARK).center();
    let radius = px(MARK / 2.0);
    let ring = px(1.2);
    let tau = std::f32::consts::TAU;
    match status {
        "done" => {
            g.paint_sector(c, px(0.0), radius, 0.0, tau, style::c(p.success));
            let k = Bounds::from_corners(point(c.x - px(3.0), c.y - px(2.5)), point(c.x + px(3.2), c.y + px(2.6)));
            style::check_mark(g, k, style::ink_on(p.success));
        }
        "blocked" => {
            g.paint_sector(c, px(0.0), radius, 0.0, tau, style::c(p.failure));
            let bar = Bounds::from_corners(point(c.x - px(3.0), c.y - px(0.8)), point(c.x + px(3.0), c.y + px(0.8)));
            style::fill_rect(g, style::snap(g, bar), style::ink_on(p.failure));
        }
        "in_progress" => {
            let blue = style::c(p.keyboard_navigation);
            g.paint_sector(c, radius - ring, radius, 0.0, tau, blue);
            // Its right half filled (from twelve o'clock round to six): half way.
            g.paint_sector(c, px(0.0), radius - ring - px(1.0), 0.0, tau / 2.0, blue);
        }
        _ => {
            let ink = style::c(olo_theme::mix(p.document_text, p.document_background, 0.55));
            g.paint_sector(c, radius - ring, radius, 0.0, tau, ink);
        }
    }
}

/// What a status reads as.
fn status_name(status: &str) -> &'static str {
    match status {
        "done" => "Done",
        "in_progress" => "In Progress",
        "blocked" => "Blocked",
        _ => "To Do",
    }
}

/// The status a click on the mark moves a task to.
fn next_status(status: &str) -> &'static str {
    match status {
        "pending" => "in_progress",
        "in_progress" => "done",
        _ => "pending",
    }
}

/// The agent's latest report on a card: what's done, what it's doing,
/// what's next, each under a coloured mark.
pub fn now_card(report: &StatusReport, running: bool) -> Element {
    let section = |kind: u8, title: &str, items: &[String]| -> Option<Element> {
        if items.is_empty() {
            return None;
        }
        let icon = canvas(move |g, b| {
            let p = theme::palette();
            match kind {
                0 => mark(g, b, "done", &p),
                1 => mark(g, b, "in_progress", &p),
                _ => {
                    // An arrow: what comes next.
                    let c = style::snap_centred(g, b.center(), MARK, MARK).center();
                    let ink = style::c(olo_theme::mix(p.panel_text, p.panel_background, 0.35));
                    g.paint_segment(point(c.x - px(4.5), c.y), point(c.x + px(4.0), c.y), px(1.4), true, ink);
                    g.paint_segment(point(c.x + px(0.5), c.y - px(3.5)), point(c.x + px(4.2), c.y), px(1.4), true, ink);
                    g.paint_segment(point(c.x + px(0.5), c.y + px(3.5)), point(c.x + px(4.2), c.y), px(1.4), true, ink);
                }
            }
        })
        .paint_key(kind as u64)
        .w(px(MARK + 2.0))
        .h(px(style::line_row(style::PLAIN_SIZE)))
        .flex_shrink_0();
        let mut list = div().flex_1().min_w_0().flex().flex_col().gap(px(2.0));
        list = list.child(StringView::new(title.to_owned()).bold(true).small(true).dim(kind != 1));
        for item in items {
            let view = StringView::new(item.clone()).wrap(true);
            list = list.child(match kind {
                0 => view.dim(true),
                _ => view,
            });
        }
        Some(div().flex().gap(px(m::TIGHT)).child(icon).child(list).into_any_element())
    };
    let when = olo_kit_base::clock::local(report.time as i64).time();
    let doing = if running { "Doing now" } else { "Was doing" };
    let mut col = div().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING));
    for s in [section(1, doing, &report.doing), section(0, "Done", &report.done), section(2, "Up next", &report.next)].into_iter().flatten() {
        col = col.child(s);
    }
    col = col.child(StringView::new(format!("As of {when}")).small(true).dim(true));
    let ground = canvas(|g, b| {
        let p = theme::palette();
        style::fill_rect(g, b, style::c(p.document_background));
        style::frame_rect(g, b, style::c(p.control_border));
    })
    .paint_key(0)
    .absolute()
    .inset_0();
    div().relative().flex_shrink_0().child(ground).child(col).into_any_element()
}

/// One line for where the conversation is narrow (no work pane): what
/// it's doing now, and what's next.
pub fn now_line(report: &StatusReport) -> Option<String> {
    let doing = report.doing.first()?;
    Some(match report.next.first() {
        Some(next) => format!("Now: {doing}  ·  Next: {next}"),
        None => format!("Now: {doing}"),
    })
}

impl Coder {
    /// The Tasks tab: the goal, how far along, the outline, a field to add.
    pub(super) fn tasks_pane(&mut self, cx: &mut Cx<Self>) -> Element {
        let Some(s) = self.session() else { return div().into_any_element() };
        let todos = s.todos.clone();
        let goal = s.goal.clone();
        let rows = Rc::new(rows(&todos, &self.folded));
        self.task_rows = rows.clone();
        let mut col = div().size_full().flex().flex_col().gap(px(m::TIGHT));
        if let Some(g) = &goal {
            let status = match g.status.as_str() {
                "complete" | "completed" | "done" => "Reached",
                "blocked" => "Blocked",
                _ => "Working toward it",
            };
            col = col.child(div().flex().flex_col().child(StringView::new(format!("Goal · {status}")).bold(true).small(true).dim(true)).child(StringView::new(g.objective.clone()).wrap(true)));
        }
        if !todos.is_empty() {
            let done = todos.iter().filter(|t| t.status == "done").count();
            col = col.child(StatusBar::new(done as f32 / todos.len() as f32).label("Tasks").trailing(format!("{done} of {} done", todos.len())));
        }
        let hits = self.task_hits.clone();
        let (cell_rows, key_rows, press_rows) = (rows.clone(), rows.clone(), rows.clone());
        let cell_hits = hits.clone();
        let list = ColumnListView::new(cx, "tasks", &self.tasks, vec![Column::new("Task", WORK_LIST_WIDTH)], rows.len(), move |_: &Coder, cell, g| {
            let Some(r) = cell_rows.get(cell.row) else { return };
            let p = theme::palette();
            let mut x = cell.bounds.left() + px(m::CELL_INSET - 2.0 + r.depth as f32 * INDENT);
            let mid = cell.bounds.center().y;
            let mut h = cell_hits.borrow_mut();
            match r.children {
                Some(open) => {
                    let b = Bounds::new(point(x, mid - px(6.0)), size(px(10.0), px(12.0)));
                    style::latch(g, b, open, cell.text_color);
                    h.latch.insert(cell.row, Bounds::from_corners(point(x - px(2.0), cell.bounds.top()), point(x + px(12.0), cell.bounds.bottom())));
                }
                None => {
                    h.latch.remove(&cell.row);
                }
            }
            x += px(12.0);
            let at = Bounds::new(point(x, mid - px(MARK / 2.0)), size(px(MARK), px(MARK)));
            mark(g, at, &r.status, &p);
            h.mark.insert(cell.row, Bounds::from_corners(point(x - px(3.0), cell.bounds.top()), point(x + px(MARK + 3.0), cell.bounds.bottom())));
            let rest = Bounds::from_corners(point(x + px(MARK + 6.0 - m::CELL_INSET), cell.bounds.top()), cell.bounds.bottom_right());
            cell.bounds = rest;
            let done = r.status == "done";
            let color = if done && !cell.selected { style::c(olo_theme::mix(p.document_text, p.document_background, 0.45)) } else { cell.text_color };
            let weight = if r.status == "in_progress" { FontWeight::SEMIBOLD } else { FontWeight::NORMAL };
            cell.text_with(g, r.text.clone(), style::ui_font(weight), color);
            cell.say(status_name(&r.status));
        })
        .no_header()
        .row_key(move |_: &Coder, row| {
            use std::hash::{Hash, Hasher};
            let mut k = std::collections::hash_map::DefaultHasher::new();
            if let Some(r) = key_rows.get(row) {
                (&r.id, &r.text, &r.status, r.depth, r.children).hash(&mut k);
            }
            k.finish()
        })
        .empty_text("No tasks yet")
        .on_press(cx.asks(move |c: &mut Coder, &(row, at): &(usize, Point<Pixels>), cx| {
            let Some(r) = press_rows.get(row) else { return false };
            let h = hits.borrow();
            if h.latch.get(&row).is_some_and(|b| b.contains(&at)) {
                drop(h);
                if !c.folded.remove(&r.id) {
                    c.folded.insert(r.id.clone());
                }
                cx.invalidate();
                return true;
            }
            if h.mark.get(&row).is_some_and(|b| b.contains(&at)) {
                c.edit_tasks(vec![json!({"edit":"change","id":r.id,"status":next_status(&r.status)})]);
                return true;
            }
            false
        }))
        .on_select(cx.listener(|_: &mut Coder, _: &(), cx| cx.invalidate()))
        .on_open(cx.msg_with(|row: &usize| Msg::TaskRename(*row)))
        .on_context(cx.msg_with(|&(row, at): &(usize, Point<Pixels>)| Msg::TaskMenu(row, at)));
        col = col.child(div().flex_1().min_h_0().child(list));
        // The chosen task in full (a row cuts a long one short), and what
        // shows it's done.
        let chosen = self.tasks.state().selected.first().and_then(|i| rows.get(*i)).cloned();
        if let Some(r) = chosen {
            let mut detail = div().flex().flex_col().child(StringView::new(r.text.clone()).wrap(true));
            let mut said = status_name(&r.status).to_owned();
            if !r.evidence.trim().is_empty() {
                said = format!("{said} · {}", r.evidence.trim());
            }
            detail = detail.child(StringView::new(said).wrap(true).dim(true).small(true));
            col = col.child(detail);
        }
        col = col.child(olo_kit::widgets::TextControl::new(cx, &self.task_field));
        col.into_any_element()
    }

    /// The person's edits, sent as one command.
    pub(super) fn edit_tasks(&self, edits: Vec<serde_json::Value>) {
        if edits.is_empty() {
            return;
        }
        self.submit(format!("/api {}", json!({"op":"edit_todos","session":self.view.id,"edits":edits})));
    }

    /// The task typed in the field: after the chosen task, beside it (at
    /// the end if none is chosen).
    pub(super) fn add_task(&mut self, cx: &mut Cx<Self>) {
        let text = self.task_field.read(cx).text().trim().to_owned();
        if text.is_empty() {
            return;
        }
        let chosen = self.tasks.state().selected.first().and_then(|i| self.task_rows.get(*i)).cloned();
        let edit = match chosen {
            Some(r) => json!({"edit":"add","text":text,"parent":r.parent,"after":r.id}),
            None => json!({"edit":"add","text":text}),
        };
        self.edit_tasks(vec![edit]);
        self.task_field.update(cx, |f, cx| f.set_text("", cx));
    }

    pub(super) fn rename_task(&mut self, row: usize, cx: &mut Cx<Self>) {
        let Some(r) = self.task_rows.get(row).cloned() else { return };
        let submission = Submission::Task { session: self.view.id.clone(), id: Some(r.id), parent: None };
        self.open_dialog("Rename Task", "What the task is, in a few words.", "Rename", vec![Field::new("Task:", r.text)], submission, cx);
    }

    pub(super) fn add_subtask(&mut self, row: usize, cx: &mut Cx<Self>) {
        let Some(r) = self.task_rows.get(row).cloned() else { return };
        let submission = Submission::Task { session: self.view.id.clone(), id: None, parent: Some(r.id) };
        self.open_dialog("Add Subtask", &format!("A step of “{}”.", r.text), "Add", vec![Field::new("Subtask:", "")], submission, cx);
    }

    /// A task's menu: its status, subtasks, wording, place, removal.
    pub(super) fn task_menu(&mut self, row: usize, at: Point<Pixels>, cx: &mut Cx<Self>) {
        let rows = self.task_rows.clone();
        let Some(r) = rows.get(row).cloned() else { return };
        // Its siblings, for moving it up and down.
        let siblings: Vec<&Row> = rows.iter().filter(|o| o.parent == r.parent).collect();
        let at_sibling = siblings.iter().position(|o| o.id == r.id).unwrap_or(0);
        let before = at_sibling.checked_sub(1).map(|i| siblings[i].id.clone());
        let after = siblings.get(at_sibling + 1).map(|o| o.id.clone());
        let grandparent = r.parent.as_ref().and_then(|p| rows.iter().find(|o| &o.id == p)).map(|o| (o.id.clone(), o.parent.clone()));
        let mut menu = Menu::new("Task");
        for status in ["pending", "in_progress", "done", "blocked"] {
            menu = menu.item(status_name(status), status).checked(status == r.status);
        }
        menu = menu.separator().item("Add Subtask…", "subtask").item("Rename…", "rename").separator();
        menu = menu.item("Move Up", "up").enabled(before.is_some()).item("Move Down", "down").enabled(after.is_some());
        menu = menu.item("Indent", "indent").enabled(before.is_some()).item("Outdent", "outdent").enabled(r.parent.is_some());
        menu = menu.separator().item("Delete", "delete");
        let me = cx.messenger();
        menu::context_menu(cx, menu, at, move |cmd, _| {
            let id = r.id.clone();
            let edit = match cmd {
                "pending" | "in_progress" | "done" | "blocked" => json!({"edit":"change","id":id,"status":cmd}),
                "subtask" => {
                    me.send(Msg::TaskSubtask(row));
                    return;
                }
                "rename" => {
                    me.send(Msg::TaskRename(row));
                    return;
                }
                "up" => json!({"edit":"move","id":id,"parent":r.parent,"before":before}),
                "down" => json!({"edit":"move","id":id,"parent":r.parent,"after":after}),
                // Under the task before it, at the end of its subtasks.
                "indent" => json!({"edit":"move","id":id,"parent":before}),
                // Beside its parent, just after it.
                "outdent" => match &grandparent {
                    Some((parent, its_parent)) => json!({"edit":"move","id":id,"parent":its_parent,"after":parent}),
                    None => return,
                },
                "delete" => json!({"edit":"remove","id":id}),
                _ => return,
            };
            me.send(Msg::TaskEdit(edit));
        });
    }
}

/// The outline's one column, as wide as the work pane holds inside its
/// tab, the list's frame and its scroll bar (so it never scrolls sideways).
pub const WORK_LIST_WIDTH: f32 = super::WORK_WIDTH - 66.0;

/// Shared between the outline's drawing and its presses.
pub type SharedHits = Rc<RefCell<Hits>>;
