use crate::client::{self, Action, Update, View};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListState, Paragraph},
};
use std::{path::PathBuf, sync::mpsc};
enum Input {
    Key(Event),
    Update { pane: usize, update: Update },
    Upgrade,
    Wrapped { pane: usize, document: std::sync::Arc<crate::rich::Text>, width: usize, lines: Vec<Line<'static>> },
}
struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen, event::DisableBracketedPaste);
    }
}
struct Pane {
    key: usize,
    client: client::Handle,
    view: View,
    draft: String,
    drafts: std::collections::HashMap<String, String>,
    cursor: usize,
    scroll: usize,
    delivery: olo_coder_core::Delivery,
    history: Vec<String>,
    history_at: usize,
    rendered: std::sync::Arc<crate::rich::Text>,
    width: usize,
    lines: Vec<Line<'static>>,
}
impl Drop for Pane {
    fn drop(&mut self) {
        let _ = self.client.tx.send(Action::Quit);
    }
}
impl Pane {
    fn open(key: usize, dir: PathBuf, workspace: PathBuf, session: Option<String>, state: serde_json::Value, updates: mpsc::Sender<Input>) -> Self {
        let draft = state["draft"].as_str().unwrap_or("").to_owned();
        let cursor = draft.len();
        let session = state["session"].as_str().filter(|s| !s.is_empty()).map(str::to_owned).or(session);
        let initial_id = session.clone().unwrap_or_default();
        Self {
            key,
            client: client::start(dir, workspace, session, move |update| {
                let _ = updates.send(Input::Update { pane: key, update });
            }),
            view: View { id: initial_id, ..Default::default() },
            draft,
            cursor,
            drafts: serde_json::from_value(state["drafts"].clone()).unwrap_or_default(),
            scroll: state["scroll"].as_u64().unwrap_or(0) as usize,
            delivery: serde_json::from_value(state["delivery"].clone()).unwrap_or_default(),
            history: Vec::new(),
            history_at: 0,
            rendered: Default::default(),
            width: 0,
            lines: Vec::new(),
        }
    }
    fn update(&mut self, u: Update) {
        match &u {
            Update::Opened(id) if id != &self.view.id => {
                if !self.view.id.is_empty() {
                    self.drafts.insert(self.view.id.clone(), std::mem::take(&mut self.draft));
                    self.scroll = 0;
                }
                if let Some(text) = self.drafts.remove(id) {
                    self.draft = text;
                }
                self.cursor = self.draft.len();
                self.lines.clear();
            }
            Update::InputResult { session, text, error: Some(_) } => {
                let draft = if session == &self.view.id { &mut self.draft } else { self.drafts.entry(session.clone()).or_default() };
                if draft.is_empty() {
                    *draft = text.clone();
                } else {
                    draft.insert_str(0, &format!("{text}\n\n"));
                }
                self.cursor = self.draft.len();
            }
            _ => {}
        }
        self.view.apply(u);
    }
    fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({"session":self.view.id,"draft":self.draft,"drafts":self.drafts,"scroll":self.scroll,"delivery":self.delivery})
    }
}
struct Picker {
    query: String,
    selected: usize,
    tile: bool,
}
fn matching(view: &View, query: &str) -> Vec<serde_json::Value> {
    let query = query.to_lowercase();
    view.catalog.iter().filter(|r| format!("{} {} {}", r["title"], r["workspace"], r["tags"]).to_lowercase().contains(&query)).cloned().collect()
}
fn mode_name(mode: olo_coder_core::Delivery) -> &'static str {
    match mode {
        olo_coder_core::Delivery::AfterTool => "After next tool",
        olo_coder_core::Delivery::Queue => "Queue after response",
        olo_coder_core::Delivery::Interrupt => "Interrupt now",
    }
}
pub fn run(dir: PathBuf, workspace: PathBuf, session: Option<String>) -> Result<()> {
    olo_kit_base::restore::claim();
    let restored = olo_kit_base::restore::take().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).unwrap_or_default();
    enable_raw_mode()?;
    let restore = Restore;
    execute!(std::io::stdout(), EnterAlternateScreen, event::EnableBracketedPaste)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let (tx, rx) = mpsc::channel();
    let (wrap, jobs) = mpsc::channel::<(usize, std::sync::Arc<crate::rich::Text>, usize)>();
    let layout_updates = tx.clone();
    std::thread::Builder::new().name("coder-terminal-layout".into()).spawn(move || {
        while let Ok(job) = jobs.recv() {
            let mut batch = std::collections::BTreeMap::new();
            batch.insert(job.0, job);
            for newer in jobs.try_iter() {
                batch.insert(newer.0, newer);
            }
            for (pane, document, width) in batch.into_values() {
                let lines = styled_lines(&document, width);
                if layout_updates.send(Input::Wrapped { pane, document, width, lines }).is_err() {
                    return;
                }
            }
        }
    })?;
    let upgrade = tx.clone();
    olo_kit_base::restore::on_upgrade(move || {
        let _ = upgrade.send(Input::Upgrade);
    });
    let events = tx.clone();
    std::thread::Builder::new().name("coder-keys".into()).spawn(move || {
        while let Ok(e) = event::read() {
            if events.send(Input::Key(e)).is_err() {
                break;
            }
        }
    })?;
    let states = restored["panes"].as_array().cloned().unwrap_or_else(|| vec![restored.clone()]);
    let mut panes = states.into_iter().take(4).enumerate().map(|(i, state)| Pane::open(i, dir.clone(), workspace.clone(), session.clone(), state, tx.clone())).collect::<Vec<_>>();
    if panes.is_empty() {
        panes.push(Pane::open(0, dir.clone(), workspace.clone(), session, serde_json::Value::Null, tx.clone()));
    }
    let mut next = panes.len();
    let mut active = (restored["active"].as_u64().unwrap_or(0) as usize).min(panes.len() - 1);
    let mut settings: Option<crate::tui_settings::Panel> = serde_json::from_value(restored["settings"].clone()).ok();
    if let Some(panel) = &mut settings {
        panel.resume();
    }
    let mut picker: Option<Picker> = None;
    let mut links: Option<Vec<(String, String)>> = None;
    let mut selected_link = 0;
    loop {
        terminal.draw(|f| {
            let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(4), Constraint::Length(4), Constraint::Length(1)]).split(f.area());
            f.render_widget(Paragraph::new(panes[active].view.status()).style(Style::default().fg(Color::Yellow)), rows[0]);
            let count = panes.len();
            let areas = if count < 3 {
                Layout::horizontal(vec![Constraint::Ratio(1, count as u32); count]).split(rows[1]).to_vec()
            } else {
                Layout::vertical([Constraint::Ratio(1, 2); 2]).split(rows[1]).iter().flat_map(|r| Layout::horizontal([Constraint::Ratio(1, 2); 2]).split(*r).to_vec()).take(count).collect()
            };
            for (i, (pane, area)) in panes.iter_mut().zip(areas).enumerate() {
                let width = area.width.saturating_sub(2).max(1) as usize;
                if !std::sync::Arc::ptr_eq(&pane.rendered, &pane.view.rich) || pane.width != width {
                    let _ = wrap.send((pane.key, pane.view.rich.clone(), width));
                    pane.rendered = pane.view.rich.clone();
                    pane.width = width;
                }
                let end = pane.lines.len().saturating_sub(pane.scroll);
                let start = end.saturating_sub(area.height.saturating_sub(2) as usize);
                let title = format!(" {} · {}{} ", i + 1, pane.view.session.as_ref().map(|s| s.title.as_str()).unwrap_or("Connecting…"), if i == active { " · active" } else { "" });
                let block = Block::bordered().title(title).border_style(Style::default().fg(if i == active { Color::LightYellow } else { Color::DarkGray }));
                f.render_widget(Paragraph::new(pane.lines[start..end].to_vec()).block(block), area);
            }
            let pane = &panes[active];
            let width = rows[2].width.saturating_sub(2).max(1) as usize;
            let before = wrapped(&pane.draft[..pane.cursor], width);
            let col = Span::raw(before.last().map(String::as_str).unwrap_or("")).width();
            let row = before.len().saturating_sub(1) + usize::from(col == width);
            let col = col % width;
            let top = row.saturating_sub(rows[2].height.saturating_sub(3) as usize);
            let input = wrapped(&pane.draft, width);
            let pending = pane.view.session.as_ref().map_or(0, |s| s.inbox.len());
            f.render_widget(
                Paragraph::new(input.get(top..).unwrap_or_default().join("\n")).block(Block::bordered().title(format!("{} · {} pending · Alt+M delivery", mode_name(pane.delivery), pending))),
                rows[2],
            );
            f.render_widget(Paragraph::new("Enter send · Ctrl+O settings · Ctrl+P sessions · Ctrl+T tile · Ctrl+N next · Esc stop · Ctrl+D detach"), rows[3]);
            if rows[2].width > 2 && rows[2].height > 2 {
                f.set_cursor_position((rows[2].x + 1 + col as u16, rows[2].y + 1 + (row - top) as u16));
            }
            if let Some(settings) = &settings {
                settings.draw(f, overlay(f.area()));
            } else if let Some(p) = &picker {
                let area = overlay(f.area());
                f.render_widget(Clear, area);
                let choices = matching(&pane.view, &p.query);
                let list = List::new(choices.iter().map(|r| format!("{}{} · {}", if r["pinned"] == true { "◆ " } else { "" }, r["title"].as_str().unwrap_or(""), r["status"].as_str().unwrap_or(""))))
                    .block(Block::bordered().title(format!("{}: {} · Alt+P pin · Esc close", if p.tile { "Tile session" } else { "Open session" }, p.query)))
                    .highlight_symbol("› ")
                    .highlight_style(Style::default().fg(Color::Black).bg(Color::LightYellow));
                f.render_stateful_widget(list, area, &mut ListState::default().with_selected(Some(p.selected)));
            } else if let Some(links) = &links {
                let area = overlay(f.area());
                f.render_widget(Clear, area);
                let list = List::new(links.iter().map(|(label, url)| format!("{label}  {url}")))
                    .block(Block::bordered().title("Links · Enter open · Esc close"))
                    .highlight_symbol("› ")
                    .highlight_style(Style::default().fg(Color::Black).bg(Color::LightYellow));
                f.render_stateful_widget(list, area, &mut ListState::default().with_selected(Some(selected_link)));
            }
        })?;
        let first = rx.recv()?;
        let mut inputs = vec![first];
        inputs.extend(rx.try_iter().take(256));
        let mut quit = false;
        for input in inputs {
            match input {
                Input::Upgrade => {
                    use std::os::unix::process::CommandExt as _;
                    let state = serde_json::json!({"active":active,"settings":settings,"panes":panes.iter().map(Pane::snapshot).collect::<Vec<_>>()}).to_string();
                    let exe = std::env::current_exe()?;
                    let exe = PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
                    let mut command = std::process::Command::new(exe);
                    command.args(std::env::args_os().skip(1));
                    olo_kit_base::restore::hand_over(&mut command, Some(&state));
                    drop(restore);
                    return Err(command.exec().into());
                }
                Input::Update { pane, update } => {
                    let focused = panes[active].key == pane;
                    if focused {
                        if let Some(panel) = &mut settings {
                            if panel.update(&update) {
                                settings = None;
                            }
                        }
                    }
                    if let Some(p) = panes.iter_mut().find(|p| p.key == pane) {
                        let selected = picker.as_ref().filter(|_| focused).and_then(|pick| matching(&p.view, &pick.query).get(pick.selected).map(|r| r["id"].clone()));
                        p.update(update);
                        if let Some(pick) = picker.as_mut().filter(|_| focused) {
                            if let Some(id) = selected {
                                if let Some(at) = matching(&p.view, &pick.query).iter().position(|r| r["id"] == id) {
                                    pick.selected = at;
                                }
                            }
                        }
                    }
                }
                Input::Wrapped { pane, document, width, lines } => {
                    if let Some(p) = panes.iter_mut().find(|p| p.key == pane) {
                        if std::sync::Arc::ptr_eq(&document, &p.view.rich) && width == p.width {
                            p.lines = lines;
                        }
                    }
                }
                Input::Key(Event::Paste(text)) => {
                    if let Some(panel) = &mut settings {
                        panel.paste(&text);
                        continue;
                    }
                    let p = &mut panes[active];
                    p.draft.insert_str(p.cursor, &text);
                    p.cursor += text.len();
                }
                Input::Key(Event::Key(k)) if k.kind != KeyEventKind::Release => {
                    if let Some(panel) = &mut settings {
                        if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
                            let _ = panes[active].client.tx.send(Action::Stop);
                        } else if panel.key(k, &panes[active].view, &panes[active].client) {
                            settings = None;
                        }
                        continue;
                    }
                    if let Some(p) = &mut picker {
                        let choices = matching(&panes[active].view, &p.query);
                        match k.code {
                            KeyCode::Esc => picker = None,
                            KeyCode::Up => p.selected = p.selected.saturating_sub(1),
                            KeyCode::Down => p.selected = (p.selected + 1).min(choices.len().saturating_sub(1)),
                            KeyCode::Char('p') if k.modifiers.contains(KeyModifiers::ALT) => {
                                if let Some(row) = choices.get(p.selected) {
                                    panes[active].client.submit(format!("/api {}", serde_json::json!({"op":"organize","session":row["id"],"pinned":row["pinned"]!=true})));
                                }
                            }
                            KeyCode::Enter => {
                                if let Some(id) = choices.get(p.selected).and_then(|r| r["id"].as_str()) {
                                    if p.tile && panes.len() < 4 {
                                        panes.push(Pane::open(next, dir.clone(), workspace.clone(), Some(id.into()), serde_json::Value::Null, tx.clone()));
                                        next += 1;
                                        active = panes.len() - 1;
                                    } else if !p.tile {
                                        panes[active].client.submit(format!("/open {id}"));
                                    } else {
                                        panes[active].view.apply(Update::Notice("Four tiles are open. Ctrl+W closes the active tile.".into()));
                                    }
                                }
                                picker = None;
                            }
                            KeyCode::Char(c) if !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                                p.query.push(c);
                                p.selected = 0;
                                let _ = panes[active].client.tx.send(Action::Browse(olo_coder_core::SessionFilter { query: p.query.clone(), ..Default::default() }));
                            }
                            KeyCode::Backspace => {
                                p.query.pop();
                                p.selected = 0;
                                let _ = panes[active].client.tx.send(Action::Browse(olo_coder_core::SessionFilter { query: p.query.clone(), ..Default::default() }));
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if let Some(choices) = &links {
                        match k.code {
                            KeyCode::Esc => links = None,
                            KeyCode::Up => selected_link = selected_link.saturating_sub(1),
                            KeyCode::Down => selected_link = (selected_link + 1).min(choices.len().saturating_sub(1)),
                            KeyCode::Enter => {
                                if let Some((_, link)) = choices.get(selected_link) {
                                    if let Some(command) = client::read_link(link) {
                                        panes[active].client.submit(command);
                                    } else if link.starts_with("https://") || link.starts_with("http://") {
                                        let link = link.clone();
                                        std::thread::spawn(move || {
                                            let _ = olo_xdg::open::open(&olo_xdg::open::Target::parse(&link));
                                        });
                                    }
                                }
                                links = None;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if k.modifiers.contains(KeyModifiers::CONTROL) {
                        match k.code {
                            KeyCode::Char('o') => {
                                settings = Some(crate::tui_settings::Panel::settings(&panes[active].view));
                                let _ = panes[active].client.tx.send(Action::Profiles);
                                continue;
                            }
                            KeyCode::Char('p') | KeyCode::Char('t') => {
                                picker = Some(Picker { query: String::new(), selected: 0, tile: k.code == KeyCode::Char('t') });
                                let _ = panes[active].client.tx.send(Action::Browse(Default::default()));
                                continue;
                            }
                            KeyCode::Char('n') => {
                                active = (active + 1) % panes.len();
                                continue;
                            }
                            KeyCode::Char('w') if panes.len() > 1 => {
                                panes.remove(active);
                                active = active.min(panes.len() - 1);
                                continue;
                            }
                            KeyCode::Char('d') => {
                                quit = true;
                                continue;
                            }
                            _ => {}
                        }
                    }
                    let p = &mut panes[active];
                    match k.code {
                        KeyCode::Char('a') if k.modifiers.contains(KeyModifiers::CONTROL) => p.cursor = 0,
                        KeyCode::Char('e') if k.modifiers.contains(KeyModifiers::CONTROL) => p.cursor = p.draft.len(),
                        KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                            p.draft.drain(..p.cursor);
                            p.cursor = 0;
                        }
                        KeyCode::Char('k') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                            p.draft.truncate(p.cursor);
                        }
                        KeyCode::Char('m') if k.modifiers.contains(KeyModifiers::ALT) => {
                            use olo_coder_core::Delivery::*;
                            p.delivery = match p.delivery {
                                AfterTool => Queue,
                                Queue => Interrupt,
                                Interrupt => AfterTool,
                            };
                        }
                        KeyCode::Char('l') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                            let mut seen = std::collections::HashSet::new();
                            links = Some(
                                p.view
                                    .rich
                                    .runs
                                    .iter()
                                    .filter_map(|r| r.style.link.as_ref().filter(|l| seen.insert((*l).clone())).map(|l| (p.view.rich.text[r.range.clone()].to_string(), l.clone())))
                                    .take(200)
                                    .collect(),
                            );
                            selected_link = 0;
                        }
                        KeyCode::Esc | KeyCode::Char('c') if k.code == KeyCode::Esc || k.modifiers.contains(KeyModifiers::CONTROL) => {
                            let _ = p.client.tx.send(Action::Stop);
                        }
                        KeyCode::Enter if k.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) => {
                            p.draft.insert(p.cursor, '\n');
                            p.cursor += 1;
                        }
                        KeyCode::Enter => {
                            if p.draft.trim() == "/quit" {
                                quit = true;
                            } else if !p.draft.trim().is_empty() {
                                p.history.push(p.draft.clone());
                                p.history_at = p.history.len();
                                let _ = p.client.tx.send(Action::Steer(std::mem::take(&mut p.draft), p.delivery));
                                p.cursor = 0;
                                p.scroll = 0;
                            }
                        }
                        KeyCode::Char(c) if !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                            p.draft.insert(p.cursor, c);
                            p.cursor += c.len_utf8();
                        }
                        KeyCode::Backspace if p.cursor > 0 => {
                            let at = p.draft[..p.cursor].char_indices().last().unwrap().0;
                            p.draft.drain(at..p.cursor);
                            p.cursor = at;
                        }
                        KeyCode::Delete if p.cursor < p.draft.len() => {
                            let end = p.cursor + p.draft[p.cursor..].chars().next().unwrap().len_utf8();
                            p.draft.drain(p.cursor..end);
                        }
                        KeyCode::Left if p.cursor > 0 => p.cursor = p.draft[..p.cursor].char_indices().last().unwrap().0,
                        KeyCode::Right if p.cursor < p.draft.len() => p.cursor += p.draft[p.cursor..].chars().next().unwrap().len_utf8(),
                        KeyCode::Home => p.cursor = 0,
                        KeyCode::End => p.cursor = p.draft.len(),
                        KeyCode::Up if p.history_at > 0 => {
                            p.history_at -= 1;
                            p.draft = p.history[p.history_at].clone();
                            p.cursor = p.draft.len();
                        }
                        KeyCode::Down if p.history_at < p.history.len() => {
                            p.history_at += 1;
                            p.draft = p.history.get(p.history_at).cloned().unwrap_or_default();
                            p.cursor = p.draft.len();
                        }
                        KeyCode::PageUp => p.scroll = p.scroll.saturating_add(10).min(p.lines.len().saturating_sub(1)),
                        KeyCode::PageDown => p.scroll = p.scroll.saturating_sub(10),
                        KeyCode::Tab => {
                            let commands = [
                                "/help",
                                "/status",
                                "/sessions",
                                "/stop",
                                "/resume",
                                "/model",
                                "/goal",
                                "/spec",
                                "/accept",
                                "/approval",
                                "/guidance",
                                "/agent",
                                "/agents",
                                "/compact",
                                "/config",
                                "/history",
                                "/memory",
                                "/git",
                                "/github",
                                "/event",
                                "/tool",
                                "/inbox",
                                "/message",
                                "/connect",
                                "/pin",
                                "/interrupt",
                                "/queue",
                                "/after-tool",
                            ];
                            if let Some(c) = commands.iter().find(|c| c.starts_with(&p.draft) && **c != p.draft) {
                                p.draft = format!("{c} ");
                                p.cursor = p.draft.len();
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        if quit {
            break;
        }
    }
    Ok(())
}
fn overlay(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).min(90);
    let height = area.height.saturating_sub(4).min(16);
    Rect::new(area.x + (area.width - width) / 2, area.y + (area.height - height) / 2, width, height)
}
/// Wrap at terminal cell boundaries, keeping combined characters and emoji intact.
fn wrapped(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for line in text.split('\n') {
        let mut row = String::new();
        let mut cells = 0;
        let span = Span::raw(expand_tabs(line, 0));
        for grapheme in span.styled_graphemes(Style::default()) {
            let count = Span::raw(grapheme.symbol).width();
            if cells + count > width && !row.is_empty() {
                out.push(std::mem::take(&mut row));
                cells = 0;
            }
            row.push_str(grapheme.symbol);
            cells += count;
        }
        out.push(row);
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_wrap_preserves_wide_combined_and_multiline_text() {
        assert_eq!(wrapped("a界b", 3), vec!["a界", "b"]);
        assert_eq!(wrapped("e\u{301}x", 1), vec!["e\u{301}", "x"]);
        assert_eq!(wrapped("hello\n\nworld\n", 8), vec!["hello", "", "world", ""]);
        assert_eq!(wrapped("👨‍👩‍👧‍👦ab", 3), vec!["👨‍👩‍👧‍👦a", "b"]);
    }
    #[test]
    fn rich_terminal_tables_align_cells_and_code_tabs_stay_indentation() {
        let mut renderer = crate::rich::Renderer::default();
        let table = renderer.render("| Check | Result |\n| --- | --- |\n| Unicode 界 | A long result that wraps across several rows |\n| Tests | 31 |\n");
        let rows = styled_lines(&table, 40);
        let rendered: Vec<_> = rows.iter().map(ToString::to_string).collect();
        let dividers: Vec<_> = rendered.iter().filter_map(|r| r.find('│').map(|at| Span::raw(&r[..at]).width())).collect();
        assert!(dividers.len() >= 4);
        assert!(dividers.iter().all(|p| *p == dividers[0]));
        assert!(rows.iter().all(|r| r.width() <= 40));
        let code = renderer.render("```rust\nfn main() {\n\tprintln!(\"hello\");\n}\n```\n");
        let lines = styled_lines(&code, 80);
        let shown = lines.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
        assert!(!shown.contains('│'));
        assert!(shown.contains("    println!"), "{shown:?}");
        assert!(lines.iter().flat_map(|l| &l.spans).any(|s| s.style.fg == Some(Color::LightMagenta)));
    }
}

fn expand_tabs(text: &str, mut column: usize) -> String {
    let mut out = String::with_capacity(text.len());
    for chunk in text.split_inclusive('\t') {
        let part = chunk.trim_end_matches('\t');
        out.push_str(part);
        column += Span::raw(part).width();
        if chunk.ends_with('\t') {
            let spaces = 4 - column % 4;
            out.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        }
    }
    out
}

fn terminal_style(run: &crate::rich::Run) -> Style {
    use crate::rich::Class;
    let mut style = Style::default();
    if run.style.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if run.style.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if run.style.strike {
        style = style.add_modifier(Modifier::CROSSED_OUT);
    }
    if run.style.link.is_some() {
        style = style.add_modifier(Modifier::UNDERLINED).fg(Color::LightBlue);
    }
    if let Some(class) = run.style.class {
        style = style.fg(match class {
            Class::Comment => Color::DarkGray,
            Class::String => Color::Green,
            Class::Keyword => Color::LightMagenta,
            Class::Number | Class::Constant => Color::LightYellow,
            Class::Type => Color::LightCyan,
            Class::Function => Color::LightBlue,
            Class::Tag => Color::Cyan,
            Class::Attribute => Color::Yellow,
            Class::Heading => Color::LightYellow,
            Class::Link => Color::LightBlue,
            Class::Plain => Color::Reset,
        });
    }
    style
}
/// Group text by style and visual row, rather than allocating one Span per glyph.
fn wrap_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut row = Vec::new();
    let mut column = 0;
    for span in spans {
        let span = Span::styled(expand_tabs(&span.content, column), span.style);
        let mut part = String::new();
        for g in span.styled_graphemes(Style::default()) {
            let symbol = g.symbol;
            let cells = Span::raw(symbol).width();
            if column + cells > width && column > 0 {
                if !part.is_empty() {
                    row.push(Span::styled(std::mem::take(&mut part), span.style));
                }
                lines.push(Line::from(std::mem::take(&mut row)));
                column = 0;
            }
            part.push_str(symbol);
            column += cells;
        }
        if !part.is_empty() {
            row.push(Span::styled(part, span.style));
        }
    }
    if !row.is_empty() || lines.is_empty() {
        lines.push(Line::from(row));
    }
    lines
}
fn styled_lines(doc: &crate::rich::Text, width: usize) -> Vec<Line<'static>> {
    use crate::rich::Kind;
    let mut lines = Vec::new();
    let mut at = 0;
    for text in doc.text.split_inclusive('\n') {
        let end = at + text.trim_end_matches('\n').len();
        let kind = doc.kind(at);
        let mut cells = vec![Vec::new()];
        for run in doc.runs(at..end) {
            let mut style = terminal_style(run);
            if matches!(kind, Kind::Heading(_)) {
                style = style.fg(Color::LightYellow).add_modifier(Modifier::BOLD);
            }
            if matches!(kind, Kind::Code(lang) if lang == "diff") {
                if text.starts_with('+') {
                    style = style.fg(Color::Green);
                } else if text.starts_with('-') {
                    style = style.fg(Color::LightRed);
                }
            }
            let fragment = &doc.text[run.range.start.max(at)..run.range.end.min(end)];
            if matches!(kind, Kind::Table { .. }) {
                for (i, part) in fragment.split('\t').enumerate() {
                    if i > 0 {
                        cells.push(Vec::new());
                    }
                    if !part.is_empty() {
                        cells.last_mut().unwrap().push(Span::styled(part.to_owned(), style));
                    }
                }
            } else {
                cells[0].push(Span::styled(fragment.to_owned(), style));
            }
        }
        if let Kind::Table { columns, header } = kind {
            let columns = (*columns).max(1);
            cells.resize_with(columns, Vec::new);
            let gap = (columns - 1) * 3;
            if width < gap + columns * 3 {
                // A narrow terminal stacks cells instead of silently clipping them.
                for cell in cells {
                    lines.extend(wrap_spans(cell, width));
                }
            } else {
                let cell_width = (width - gap) / columns;
                let wrapped: Vec<_> = cells.into_iter().map(|c| wrap_spans(c, cell_width)).collect();
                for row in 0..wrapped.iter().map(Vec::len).max().unwrap_or(1) {
                    let mut spans = Vec::new();
                    for (i, cell) in wrapped.iter().enumerate() {
                        if i > 0 {
                            spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
                        }
                        let line = cell.get(row).cloned().unwrap_or_default();
                        let padding = cell_width.saturating_sub(line.width());
                        spans.extend(line.spans);
                        spans.push(Span::raw(" ".repeat(padding)));
                    }
                    lines.push(Line::from(spans));
                }
                if *header {
                    lines.push(Line::styled((0..columns).map(|_| "─".repeat(cell_width)).collect::<Vec<_>>().join("─┼─"), Style::default().fg(Color::DarkGray)));
                }
            }
        } else {
            lines.extend(wrap_spans(cells.remove(0), width));
        }
        at += text.len();
    }
    lines
}

#[cfg(test)]
mod session_tests {
    use super::*;
    #[test]
    fn switching_tiles_preserves_drafts_and_failed_input() {
        let (tx, _) = mpsc::channel();
        let mut pane = Pane {
            key: 0,
            client: client::Handle { tx: tx.into() },
            view: View::default(),
            draft: String::new(),
            drafts: Default::default(),
            cursor: 0,
            scroll: 0,
            delivery: Default::default(),
            history: vec![],
            history_at: 0,
            rendered: Default::default(),
            width: 0,
            lines: vec![],
        };
        pane.update(Update::Opened("first".into()));
        pane.draft = "unfinished λ".into();
        pane.cursor = pane.draft.len();
        pane.update(Update::Opened("second".into()));
        assert!(pane.draft.is_empty());
        pane.draft = "another task".into();
        pane.update(Update::InputResult { session: "first".into(), text: "failed submission".into(), error: Some("offline".into()) });
        assert_eq!(pane.draft, "another task");
        pane.update(Update::Opened("first".into()));
        assert_eq!(pane.draft, "failed submission\n\nunfinished λ");
        assert_eq!(pane.cursor, pane.draft.len());
        pane.update(Update::Opened("second".into()));
        assert_eq!(pane.draft, "another task");
        let state = pane.snapshot();
        assert_eq!(state["drafts"]["first"], "failed submission\n\nunfinished λ");
    }
}
