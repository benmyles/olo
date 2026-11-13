//! The task list in the terminal (Alt+T): the same tree the window shows,
//! edited the same way (each change one `edit_todos` command the agent is
//! told about), from the keyboard.
use crate::client::Handle;
use olo_coder_core::{StatusReport, Todo};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};
use serde_json::{Value, json};

/// What's being typed, and what it becomes.
enum Typing {
    Rename(String),
    /// A new task beside `after` (under `parent`), or at the end.
    Add {
        parent: Option<String>,
        after: Option<String>,
    },
    Subtask(String),
}

#[derive(Default)]
pub struct Panel {
    selected: usize,
    typing: Option<(Typing, String)>,
}

/// Each task with how deep it is.
fn depths(todos: &[Todo]) -> Vec<usize> {
    let mut depth = std::collections::HashMap::new();
    todos
        .iter()
        .map(|t| {
            let d = t.parent.as_deref().and_then(|p| depth.get(p).copied()).map_or(0, |d: usize| d + 1);
            depth.insert(t.id.as_str(), d);
            d
        })
        .collect()
}

/// A task's mark, coloured by its status.
pub fn mark(status: &str) -> Span<'static> {
    match status {
        "done" => Span::styled("✓", Style::default().fg(Color::Green)),
        "in_progress" => Span::styled("◐", Style::default().fg(Color::LightBlue)),
        "blocked" => Span::styled("✕", Style::default().fg(Color::LightRed)),
        _ => Span::styled("○", Style::default().fg(Color::DarkGray)),
    }
}

/// The agent's report in one line: what it's doing now, and what's next.
pub fn now_line(report: &StatusReport, running: bool) -> Line<'static> {
    let mut spans = Vec::new();
    if let Some(doing) = report.doing.first() {
        spans.push(Span::styled(if running { "◐ Now: " } else { "◐ Was: " }, Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD)));
        spans.push(Span::raw(doing.clone()));
    } else if let Some(done) = report.done.first() {
        spans.push(Span::styled("✓ Done: ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)));
        spans.push(Span::raw(done.clone()));
    }
    if let Some(next) = report.next.first() {
        spans.push(Span::styled("   → Next: ", Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(next.clone(), Style::default().fg(Color::Gray)));
    }
    Line::from(spans)
}

impl Panel {
    pub fn draw(&self, f: &mut Frame, area: Rect, todos: &[Todo], report: Option<&StatusReport>) {
        f.render_widget(Clear, area);
        let done = todos.iter().filter(|t| t.status == "done").count();
        let title = format!(" Tasks · {done} of {} done · Space status · Enter rename · a add · s subtask · Tab/⇧Tab nest · Alt+↑↓ move · Del remove · Esc close ", todos.len());
        let block = Block::bordered().title(title).border_style(Style::default().fg(Color::LightYellow));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let mut lines: Vec<Line> = Vec::new();
        if let Some(r) = report {
            for (head, color, items) in [("Doing now", Color::LightBlue, &r.doing), ("Done", Color::Green, &r.done), ("Up next", Color::Gray, &r.next)] {
                if !items.is_empty() {
                    lines.push(Line::from(vec![Span::styled(format!("{head}: "), Style::default().fg(color).add_modifier(Modifier::BOLD)), Span::raw(items.join(" · "))]));
                }
            }
            lines.push(Line::default());
        }
        let top = lines.len();
        if todos.is_empty() {
            lines.push(Line::styled("No tasks yet: the agent lists them here as it works. Press a to add one.", Style::default().fg(Color::DarkGray)));
        }
        for (i, (t, d)) in todos.iter().zip(depths(todos)).enumerate() {
            let selected = i == self.selected;
            let mut text_style = if t.status == "done" { Style::default().fg(Color::DarkGray) } else { Style::default() };
            if t.status == "in_progress" {
                text_style = text_style.add_modifier(Modifier::BOLD);
            }
            if selected {
                text_style = text_style.fg(Color::Black).bg(Color::LightYellow);
            }
            let text = match &self.typing {
                Some((Typing::Rename(id), typed)) if *id == t.id => format!("{typed}▏"),
                _ => t.text.clone(),
            };
            lines.push(Line::from(vec![Span::raw(format!("{}{} ", if selected { "›" } else { " " }, "  ".repeat(d))), mark(&t.status), Span::raw(" "), Span::styled(text, text_style)]));
            let adding_here = match &self.typing {
                Some((Typing::Add { after: Some(a), .. }, _)) => *a == t.id,
                Some((Typing::Subtask(p), _)) => *p == t.id,
                _ => false,
            };
            if adding_here {
                let extra = if matches!(self.typing, Some((Typing::Subtask(_), _))) { 1 } else { 0 };
                let typed = self.typing.as_ref().map_or("", |(_, s)| s.as_str());
                lines.push(Line::from(vec![Span::raw(format!("  {}", "  ".repeat(d + extra))), mark("pending"), Span::raw(format!(" {typed}▏"))]));
            }
        }
        if let Some((Typing::Add { after: None, .. }, typed)) = &self.typing {
            lines.push(Line::from(vec![Span::raw("  "), mark("pending"), Span::raw(format!(" {typed}▏"))]));
        }
        // Keep the chosen task in view.
        let height = inner.height as usize;
        let chosen = top + self.selected;
        let skip = if lines.len() <= height { 0 } else { (chosen + 2).saturating_sub(height).min(lines.len() - height) };
        f.render_widget(Paragraph::new(lines.into_iter().skip(skip).collect::<Vec<_>>()), inner);
    }

    /// A key; true when the panel is done with.
    pub fn key(&mut self, k: KeyEvent, todos: &[Todo], session: &str, client: &Handle) -> bool {
        let edit = |edits: Vec<Value>| client.submit(format!("/api {}", json!({"op":"edit_todos","session":session,"edits":edits})));
        self.selected = self.selected.min(todos.len().saturating_sub(1));
        if let Some((typing, text)) = &mut self.typing {
            match k.code {
                KeyCode::Esc => self.typing = None,
                KeyCode::Enter => {
                    let text = text.trim().to_owned();
                    if !text.is_empty() {
                        edit(vec![match typing {
                            Typing::Rename(id) => json!({"edit":"change","id":id,"text":text}),
                            Typing::Add { parent, after } => json!({"edit":"add","text":text,"parent":parent,"after":after}),
                            Typing::Subtask(parent) => json!({"edit":"add","text":text,"parent":parent}),
                        }]);
                    }
                    self.typing = None;
                }
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => text.clear(),
                KeyCode::Char(c) if !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => text.push(c),
                _ => {}
            }
            return false;
        }
        let chosen = todos.get(self.selected);
        let siblings: Vec<&Todo> = chosen.map(|c| todos.iter().filter(|t| t.parent == c.parent).collect()).unwrap_or_default();
        let at = chosen.and_then(|c| siblings.iter().position(|t| t.id == c.id)).unwrap_or(0);
        let before = at.checked_sub(1).and_then(|i| siblings.get(i)).map(|t| t.id.clone());
        let after = siblings.get(at + 1).map(|t| t.id.clone());
        match k.code {
            KeyCode::Esc => return true,
            KeyCode::Char('t') if k.modifiers.contains(KeyModifiers::ALT) => return true,
            KeyCode::Up if k.modifiers.contains(KeyModifiers::ALT) => {
                if let (Some(c), Some(b)) = (chosen, before) {
                    edit(vec![json!({"edit":"move","id":c.id,"parent":c.parent,"before":b})]);
                    self.selected = todos.iter().position(|t| t.id == b).unwrap_or(self.selected);
                }
            }
            KeyCode::Down if k.modifiers.contains(KeyModifiers::ALT) => {
                if let (Some(c), Some(a)) = (chosen, after) {
                    edit(vec![json!({"edit":"move","id":c.id,"parent":c.parent,"after":a})]);
                    let i = todos.iter().position(|t| t.id == a).unwrap_or(self.selected);
                    self.selected = crate::tui_tasks::subtree_end(todos, i).saturating_sub(crate::tui_tasks::subtree_end(todos, self.selected) - self.selected);
                }
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected = (self.selected + 1).min(todos.len().saturating_sub(1)),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = todos.len().saturating_sub(1),
            KeyCode::Char(' ') => {
                if let Some(c) = chosen {
                    let next = match c.status.as_str() {
                        "pending" => "in_progress",
                        "in_progress" => "done",
                        _ => "pending",
                    };
                    edit(vec![json!({"edit":"change","id":c.id,"status":next})]);
                }
            }
            KeyCode::Char('b') => {
                if let Some(c) = chosen {
                    edit(vec![json!({"edit":"change","id":c.id,"status":if c.status == "blocked" { "pending" } else { "blocked" }})]);
                }
            }
            KeyCode::Enter => {
                if let Some(c) = chosen {
                    self.typing = Some((Typing::Rename(c.id.clone()), c.text.clone()));
                }
            }
            KeyCode::Char('a') => {
                self.typing = Some((Typing::Add { parent: chosen.and_then(|c| c.parent.clone()), after: chosen.map(|c| c.id.clone()) }, String::new()));
            }
            KeyCode::Char('s') => {
                if let Some(c) = chosen {
                    self.typing = Some((Typing::Subtask(c.id.clone()), String::new()));
                }
            }
            KeyCode::Tab => {
                if let (Some(c), Some(b)) = (chosen, before) {
                    edit(vec![json!({"edit":"move","id":c.id,"parent":b})]);
                }
            }
            KeyCode::BackTab => {
                if let Some(c) = chosen {
                    if let Some(parent) = c.parent.as_ref().and_then(|p| todos.iter().find(|t| &t.id == p)) {
                        edit(vec![json!({"edit":"move","id":c.id,"parent":parent.parent,"after":parent.id})]);
                    }
                }
            }
            KeyCode::Delete | KeyCode::Char('d') => {
                if let Some(c) = chosen {
                    edit(vec![json!({"edit":"remove","id":c.id})]);
                }
            }
            _ => {}
        }
        false
    }
}

/// Where task `i`'s subtasks end.
fn subtree_end(todos: &[Todo], i: usize) -> usize {
    if i >= todos.len() { todos.len() } else { olo_coder_core::todos::subtree_end(todos, i) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Action;
    use ratatui::crossterm::event::KeyEventState;

    fn todo(id: &str, parent: Option<&str>, status: &str, text: &str) -> Todo {
        Todo { id: id.into(), text: text.into(), status: status.into(), evidence: String::new(), parent: parent.map(Into::into) }
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent { code, modifiers, kind: ratatui::crossterm::event::KeyEventKind::Press, state: KeyEventState::NONE }
    }

    fn sent(rx: &std::sync::mpsc::Receiver<Action>) -> Value {
        match rx.try_recv() {
            Ok(Action::Submit(text)) => serde_json::from_str(text.trim_start_matches("/api ")).unwrap(),
            _ => Value::Null,
        }
    }

    #[test]
    fn shows_the_tree_and_report_and_edits_from_the_keyboard() {
        let todos =
            vec![todo("t1", None, "in_progress", "Build the parser"), todo("t2", Some("t1"), "done", "Tokenizer"), todo("t3", Some("t1"), "pending", "Grammar"), todo("t4", None, "pending", "Docs")];
        let report = StatusReport { done: vec!["Wrote the tokenizer".into()], doing: vec!["Writing the grammar".into()], next: vec!["Docs".into()], time: 0, sequence: 0 };
        let mut panel = Panel::default();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 14)).unwrap();
        terminal.draw(|f| panel.draw(f, f.area(), &todos, Some(&report))).unwrap();
        let buf = terminal.backend().buffer();
        let screen: String = (0..14).map(|y| (0..100).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n");
        assert!(screen.contains("Doing now: Writing the grammar"), "{screen}");
        assert!(screen.contains("› ◐ Build the parser"), "{screen}");
        assert!(screen.contains("    ✓ Tokenizer"), "subtasks are set in: {screen}");

        let (tx, rx) = std::sync::mpsc::channel();
        let client = Handle { tx: tx.into() };
        // Space moves the chosen task on; Tab nests it under the one before.
        panel.key(key(KeyCode::Down, KeyModifiers::NONE), &todos, "s", &client);
        panel.key(key(KeyCode::Down, KeyModifiers::NONE), &todos, "s", &client);
        panel.key(key(KeyCode::Char(' '), KeyModifiers::NONE), &todos, "s", &client);
        assert_eq!(sent(&rx)["edits"][0], json!({"edit":"change","id":"t3","status":"in_progress"}));
        panel.key(key(KeyCode::Tab, KeyModifiers::NONE), &todos, "s", &client);
        assert_eq!(sent(&rx)["edits"][0], json!({"edit":"move","id":"t3","parent":"t2"}));
        panel.key(key(KeyCode::BackTab, KeyModifiers::NONE), &todos, "s", &client);
        assert_eq!(sent(&rx)["edits"][0], json!({"edit":"move","id":"t3","parent":null,"after":"t1"}));
        // a, typing, Enter: a new task beside the chosen one.
        panel.key(key(KeyCode::Char('a'), KeyModifiers::NONE), &todos, "s", &client);
        for c in "Tests".chars() {
            panel.key(key(KeyCode::Char(c), KeyModifiers::NONE), &todos, "s", &client);
        }
        panel.key(key(KeyCode::Enter, KeyModifiers::NONE), &todos, "s", &client);
        assert_eq!(sent(&rx)["edits"][0], json!({"edit":"add","text":"Tests","parent":"t1","after":"t3"}));
        assert!(panel.key(key(KeyCode::Esc, KeyModifiers::NONE), &todos, "s", &client), "Esc closes it");
    }
}
