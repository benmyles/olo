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
    widgets::{Block, Clear, List, ListState, Paragraph, Wrap},
};
use std::{path::PathBuf, sync::mpsc};
enum Input {
    Key(Event),
    Update(Update),
    Upgrade,
    Wrapped { document: std::sync::Arc<olo_ui::rich_text::Text>, width: usize, lines: Vec<Line<'static>> },
}
struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen, event::DisableBracketedPaste);
    }
}
pub fn run(dir: PathBuf, workspace: PathBuf, session: Option<String>) -> Result<()> {
    olo_kit_base::restore::claim();
    let restored = olo_kit_base::restore::take().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    let session = restored.as_ref().and_then(|v| v["session"].as_str()).filter(|s| !s.is_empty()).map(str::to_owned).or(session);
    enable_raw_mode()?;
    let restore = Restore;
    execute!(std::io::stdout(), EnterAlternateScreen, event::EnableBracketedPaste)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let (tx, rx) = mpsc::channel();
    let (wrap, jobs) = mpsc::channel::<(std::sync::Arc<olo_ui::rich_text::Text>, usize)>();
    let layout_updates = tx.clone();
    std::thread::Builder::new().name("coder-terminal-layout".into()).spawn(move || {
        while let Ok(mut job) = jobs.recv() {
            for newer in jobs.try_iter() {
                job = newer;
            }
            let (document, width) = job;
            let lines = styled_lines(&document, width);
            if layout_updates.send(Input::Wrapped { document, width, lines }).is_err() {
                break;
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
    let client = client::start(dir, workspace, session, move |u| {
        let _ = tx.send(Input::Update(u));
    });
    let mut view = View::default();
    view.push("Olo Coder\nA persistent workspace for your agent. /help lists commands.\nCtrl+D detaches; work continues. Escape stops the current turn.");
    let mut draft = restored.as_ref().and_then(|v| v["draft"].as_str()).unwrap_or("").to_string();
    let mut cursor = draft.len();
    let mut scroll = restored.as_ref().and_then(|v| v["scroll"].as_u64()).unwrap_or(0) as usize;
    let mut history = Vec::<String>::new();
    let mut history_at = 0;
    let mut rendered = std::sync::Arc::default();
    let mut render_width = 0;
    let mut rich_lines = Vec::new();
    let mut links: Option<Vec<(String, String)>> = None;
    let mut selected_link = 0;
    loop {
        terminal.draw(|f| {
            let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(4), Constraint::Length(4), Constraint::Length(1)]).split(f.area());
            f.render_widget(Paragraph::new(view.status()).style(Style::default().fg(Color::Yellow)), rows[0]);
            let panes = Layout::horizontal([Constraint::Percentage(72), Constraint::Percentage(28)]).split(rows[1]);
            let width = panes[0].width.saturating_sub(2).max(1) as usize;
            if !std::sync::Arc::ptr_eq(&rendered, &view.rich) || render_width != width {
                let _ = wrap.send((view.rich.clone(), width));
                rendered = view.rich.clone();
                render_width = width;
            }
            let lines = &rich_lines;
            let visible = panes[0].height.saturating_sub(2) as usize;
            let end = lines.len().saturating_sub(scroll);
            let start = end.saturating_sub(visible);
            f.render_widget(Paragraph::new(lines[start..end].to_vec()).block(Block::bordered().title("Conversation")), panes[0]);
            f.render_widget(Paragraph::new(view.work()).wrap(Wrap { trim: false }).block(Block::bordered().title("Work")), panes[1]);
            let width = rows[2].width.saturating_sub(2).max(1) as usize;
            let before = wrapped(&draft[..cursor], width);
            let col = Span::raw(before.last().map(String::as_str).unwrap_or("")).width();
            let row = before.len().saturating_sub(1) + usize::from(col == width);
            let col = col % width;
            let top = row.saturating_sub(rows[2].height.saturating_sub(3) as usize);
            let input = wrapped(&draft, width);
            f.render_widget(Paragraph::new(input.get(top..).unwrap_or_default().join("\n")).block(Block::bordered().title("Message or /command")), rows[2]);
            f.render_widget(Paragraph::new("Enter send · Alt+Enter newline · Esc stop · Ctrl+O links · Ctrl+D detach"), rows[3]);
            if rows[2].width > 2 && rows[2].height > 2 {
                f.set_cursor_position((rows[2].x + 1 + col as u16, rows[2].y + 1 + (row - top) as u16));
            }
            if let Some(links) = &links {
                let area = f.area();
                let width = area.width.saturating_sub(4).min(90);
                let height = area.height.saturating_sub(4).min(16);
                let area = Rect::new(area.x + (area.width - width) / 2, area.y + (area.height - height) / 2, width, height);
                f.render_widget(Clear, area);
                let list = List::new(links.iter().map(|(label, url)| format!("{label}  {url}")))
                    .block(Block::bordered().title("Links · Enter open · Esc close"))
                    .highlight_symbol("› ")
                    .highlight_style(Style::default().fg(Color::Black).bg(Color::LightYellow).add_modifier(Modifier::BOLD));
                f.render_stateful_widget(list, area, &mut ListState::default().with_selected(Some(selected_link)));
            }
        })?;
        // No polling, no idle frames. Fold a burst before drawing once.
        let first = rx.recv()?;
        let mut inputs = vec![first];
        inputs.extend(rx.try_iter().take(256));
        let mut quit = false;
        for input in inputs {
            if matches!(&input, Input::Upgrade) {
                use std::os::unix::process::CommandExt as _;
                let state = serde_json::json!({"session":view.id,"draft":draft,"scroll":scroll}).to_string();
                let exe = std::env::current_exe()?;
                let exe = PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
                let mut command = std::process::Command::new(exe);
                command.args(std::env::args_os().skip(1));
                olo_kit_base::restore::hand_over(&mut command, Some(&state));
                drop(restore);
                return Err(command.exec().into());
            }
            if let Input::Key(Event::Key(k)) = &input {
                if k.kind != KeyEventKind::Release {
                    if let Some(choices) = &links {
                        match k.code {
                            KeyCode::Esc => links = None,
                            KeyCode::Up => selected_link = selected_link.saturating_sub(1),
                            KeyCode::Down => selected_link = (selected_link + 1).min(choices.len().saturating_sub(1)),
                            KeyCode::Enter => {
                                if let Some((_, link)) = choices.get(selected_link) {
                                    if let Some(id) = link.strip_prefix("olo-coder:event/").and_then(|s| s.parse::<i64>().ok()) {
                                        client.submit(format!("/event {id}"));
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
                }
            }
            match input {
                Input::Update(u) => view.apply(u),
                Input::Wrapped { document, width, lines } => {
                    if std::sync::Arc::ptr_eq(&document, &view.rich) && width == render_width {
                        rich_lines = lines;
                    }
                }
                Input::Key(Event::Paste(text)) => {
                    draft.insert_str(cursor, &text);
                    cursor += text.len();
                }
                Input::Key(Event::Key(k)) if k.kind != KeyEventKind::Release => match k.code {
                    KeyCode::Char('o') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        let mut seen = std::collections::HashSet::new();
                        links = Some(
                            view.rich
                                .runs
                                .iter()
                                .filter_map(|r| r.style.link.as_ref().filter(|l| seen.insert((*l).clone())).map(|l| (view.rich.text[r.range.clone()].to_string(), l.clone())))
                                .take(200)
                                .collect(),
                        );
                        selected_link = 0;
                    }
                    KeyCode::Char('d') if k.modifiers.contains(KeyModifiers::CONTROL) => quit = true,
                    KeyCode::Esc | KeyCode::Char('c') if k.code == KeyCode::Esc || k.modifiers.contains(KeyModifiers::CONTROL) => {
                        let _ = client.tx.send(Action::Stop);
                    }
                    KeyCode::Enter if k.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) => {
                        draft.insert(cursor, '\n');
                        cursor += 1;
                    }
                    KeyCode::Enter => {
                        if draft.trim() == "/quit" {
                            quit = true;
                        } else if !draft.trim().is_empty() {
                            history.push(draft.clone());
                            history_at = history.len();
                            client.submit(std::mem::take(&mut draft));
                            cursor = 0;
                            scroll = 0;
                        }
                    }
                    KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                        draft.insert(cursor, c);
                        cursor += c.len_utf8();
                    }
                    KeyCode::Backspace if cursor > 0 => {
                        let at = draft[..cursor].char_indices().last().unwrap().0;
                        draft.drain(at..cursor);
                        cursor = at;
                    }
                    KeyCode::Delete if cursor < draft.len() => {
                        let end = cursor + draft[cursor..].chars().next().unwrap().len_utf8();
                        draft.drain(cursor..end);
                    }
                    KeyCode::Left if cursor > 0 => cursor = draft[..cursor].char_indices().last().unwrap().0,
                    KeyCode::Right if cursor < draft.len() => cursor += draft[cursor..].chars().next().unwrap().len_utf8(),
                    KeyCode::Home => cursor = 0,
                    KeyCode::End => cursor = draft.len(),
                    KeyCode::Up if history_at > 0 => {
                        history_at -= 1;
                        draft = history[history_at].clone();
                        cursor = draft.len();
                    }
                    KeyCode::Down if history_at < history.len() => {
                        history_at += 1;
                        draft = history.get(history_at).cloned().unwrap_or_default();
                        cursor = draft.len();
                    }
                    KeyCode::PageUp => scroll = scroll.saturating_add(10).min(rich_lines.len().saturating_sub(1)),
                    KeyCode::PageDown => scroll = scroll.saturating_sub(10),
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
                            "/agents",
                            "/compact",
                            "/config",
                            "/history",
                            "/memory",
                            "/git",
                            "/github",
                            "/event",
                            "/tool",
                        ];
                        if let Some(c) = commands.iter().find(|c| c.starts_with(&draft) && **c != draft) {
                            draft = format!("{c} ");
                            cursor = draft.len();
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        if quit {
            break;
        }
    }
    let _ = client.tx.send(Action::Quit);
    Ok(())
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
        let mut renderer = olo_ui::rich_text::Renderer::default();
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

fn terminal_style(run: &olo_ui::rich_text::Run) -> Style {
    use olo_ui::rich_text::Class;
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
fn styled_lines(doc: &olo_ui::rich_text::Text, width: usize) -> Vec<Line<'static>> {
    use olo_ui::rich_text::Kind;
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
