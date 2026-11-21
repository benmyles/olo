//! Full Vim/tmux-style Copy Mode (`⇧⌘C`) state machine over `olo_rex_vt::GhosttyTerminal`.
//!
//! Supports all motions (`h/j/k/l`, `w/b/e/W/B/E`, `0/^/$`, `H/M/L`,
//! `Ctrl-U/D/B/F`, `g/G`, `{/}`), numeric counts (`1..9999`), visual
//! character/line/rectangular-block selection (`v`/`V`/`Ctrl-V`), endpoint swap
//! (`o`), yank (`y`/`Enter`/`D`), search (`/`/`?`/`n`/`N`), and inline character
//! find (`f`/`F`/`t`/`T`/`;`/`,`).

use olo_rex_vt::GhosttyTerminal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualKind {
    Char,
    Line,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharFindKind {
    Forward,
    Backward,
    TillForward,
    TillBackward,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyModeAction {
    Continue,
    Exit,
    Yank(String),
}

#[derive(Debug, Clone)]
pub struct CopyMode {
    /// Cursor column in viewport (0-based).
    pub col: u16,
    /// Cursor row in viewport (0-based).
    pub row: u32,
    /// Selection anchor in absolute screen coordinates `(col, abs_row)` where
    /// `abs_row = scrollbar.offset + viewport_row`.
    pub anchor: Option<(u16, u64)>,
    pub visual: Option<VisualKind>,
    /// Numeric count prefix accumulator (`3j`, `10w`, etc.).
    pub count_buf: String,
    /// Pending two-key operator (`g`, `f`, `F`, `t`, `T`).
    pub pending: Option<char>,
    /// Active search prompt (`/` forward or `?` backward).
    pub search_prompt: Option<(bool, String)>,
    pub last_search: Option<(bool, String)>,
    pub last_char_find: Option<(CharFindKind, char)>,
}

impl CopyMode {
    pub fn enter(term: &GhosttyTerminal, cursor_col: u16, cursor_row: u16) -> Self {
        let (cols, rows) = term.size();
        Self {
            col: cursor_col.min(cols.saturating_sub(1)),
            row: (cursor_row as u32).min(rows.saturating_sub(1) as u32),
            anchor: None,
            visual: None,
            count_buf: String::new(),
            pending: None,
            search_prompt: None,
            last_search: None,
            last_char_find: None,
        }
    }

    pub fn status_text(&self) -> String {
        if let Some((fwd, q)) = &self.search_prompt {
            return format!("{}{}", if *fwd { "/" } else { "?" }, q);
        }
        let mode = match self.visual {
            None => "COPY",
            Some(VisualKind::Char) => "COPY VISUAL",
            Some(VisualKind::Line) => "COPY LINE",
            Some(VisualKind::Block) => "COPY BLOCK",
        };
        if self.count_buf.is_empty() { format!("{mode}  Ln {}, Col {}", self.row + 1, self.col + 1) } else { format!("{mode}  {}  Ln {}, Col {}", self.count_buf, self.row + 1, self.col + 1) }
    }

    fn take_count(&mut self) -> usize {
        if self.count_buf.is_empty() {
            1
        } else {
            let n = self.count_buf.parse::<usize>().unwrap_or(1).clamp(1, 10_000);
            self.count_buf.clear();
            n
        }
    }

    fn abs_row(&self, term: &GhosttyTerminal) -> u64 {
        term.scrollbar().offset + self.row as u64
    }

    /// Handle a keystroke in Copy Mode, updating viewport scroll & selection directly on `term`.
    pub fn handle_key(&mut self, term: &mut GhosttyTerminal, key: &str, text: Option<&str>, ctrl: bool, shift: bool) -> CopyModeAction {
        let (cols, rows) = term.size();
        if cols == 0 || rows == 0 {
            return CopyModeAction::Exit;
        }

        // Interactive search bar inside Copy Mode (`/` or `?`).
        if let Some((fwd, mut q)) = self.search_prompt.take() {
            match key {
                "escape" => return CopyModeAction::Continue,
                "enter" => {
                    if !q.is_empty() {
                        self.last_search = Some((fwd, q.clone()));
                        self.jump_search(term, fwd, &q);
                        self.sync_selection(term);
                    }
                    return CopyModeAction::Continue;
                }
                "backspace" => {
                    q.pop();
                    self.search_prompt = Some((fwd, q));
                    return CopyModeAction::Continue;
                }
                _ => {
                    if let Some(t) = text
                        && !ctrl
                    {
                        q.push_str(t);
                    }
                    self.search_prompt = Some((fwd, q));
                    return CopyModeAction::Continue;
                }
            }
        }

        // Two-key pending operator (`f/F/t/T` or `g`).
        if let Some(op) = self.pending.take() {
            if op == 'g' {
                if text == Some("g") {
                    let _ = term.scroll_to_top();
                    self.row = 0;
                    self.col = 0;
                    self.count_buf.clear();
                    self.sync_selection(term);
                }
                return CopyModeAction::Continue;
            }
            if let Some(ch) = text.and_then(|s| s.chars().next()) {
                let kind = match op {
                    'f' => CharFindKind::Forward,
                    'F' => CharFindKind::Backward,
                    't' => CharFindKind::TillForward,
                    _ => CharFindKind::TillBackward,
                };
                self.last_char_find = Some((kind, ch));
                let n = self.take_count();
                for _ in 0..n {
                    self.find_char_in_line(term, kind, ch);
                }
                self.sync_selection(term);
            }
            return CopyModeAction::Continue;
        }

        // Exit keys
        if key == "escape" || (key == "q" && !ctrl) || (ctrl && key == "c") {
            let _ = term.clear_selection();
            return CopyModeAction::Exit;
        }

        // Numeric count prefix (`0` starts line-start if no count yet).
        if !ctrl
            && let Some(ch) = text.and_then(|s| s.chars().next())
            && ch.is_ascii_digit()
            && (ch != '0' || !self.count_buf.is_empty())
        {
            if self.count_buf.len() < 4 {
                self.count_buf.push(ch);
            }
            return CopyModeAction::Continue;
        }

        // Ctrl motions & block visual
        if ctrl {
            let half = (rows / 2).max(1) as isize;
            let page = rows.max(1) as isize;
            match key {
                "u" => {
                    let n = self.take_count() as isize;
                    let _ = term.scroll_delta(-half * n);
                    self.sync_selection(term);
                    return CopyModeAction::Continue;
                }
                "d" => {
                    let n = self.take_count() as isize;
                    let _ = term.scroll_delta(half * n);
                    self.sync_selection(term);
                    return CopyModeAction::Continue;
                }
                "b" => {
                    let n = self.take_count() as isize;
                    let _ = term.scroll_delta(-page * n);
                    self.sync_selection(term);
                    return CopyModeAction::Continue;
                }
                "f" => {
                    let n = self.take_count() as isize;
                    let _ = term.scroll_delta(page * n);
                    self.sync_selection(term);
                    return CopyModeAction::Continue;
                }
                "v" => {
                    self.toggle_visual(term, VisualKind::Block);
                    self.sync_selection(term);
                    return CopyModeAction::Continue;
                }
                _ => return CopyModeAction::Continue,
            }
        }

        let ch = text.and_then(|s| s.chars().next());
        match (key, ch) {
            ("enter", _) | (_, Some('y')) => {
                if self.visual.is_none() {
                    let _ = term.select_line_at_viewport(self.col, self.row);
                } else {
                    self.sync_selection(term);
                }
                let copied = term.selection_text().ok().flatten().unwrap_or_default();
                let _ = term.clear_selection();
                return CopyModeAction::Yank(copied);
            }
            (_, Some('D')) => {
                let line = term.viewport_row_text(self.row);
                let end_col = line.chars().count().saturating_sub(1) as u16;
                let _ = term.set_viewport_selection((self.col, self.row), (end_col.max(self.col), self.row), false);
                let copied = term.selection_text().ok().flatten().unwrap_or_default();
                let _ = term.clear_selection();
                return CopyModeAction::Yank(copied);
            }
            (_, Some('v')) => self.toggle_visual(term, VisualKind::Char),
            (_, Some('V')) => self.toggle_visual(term, VisualKind::Line),
            (_, Some('o')) => {
                if let Some((ac, ar)) = self.anchor {
                    let cur_abs = self.abs_row(term);
                    let cur_col = self.col;
                    let sb = term.scrollbar();
                    if ar >= sb.offset && ar < sb.offset + rows as u64 {
                        self.col = ac;
                        self.row = (ar - sb.offset) as u32;
                        self.anchor = Some((cur_col, cur_abs));
                    }
                }
            }
            ("left", _) | (_, Some('h')) => {
                let n = self.take_count() as u16;
                self.col = self.col.saturating_sub(n);
            }
            ("right", _) | (_, Some('l')) => {
                let n = self.take_count() as u16;
                self.col = (self.col + n).min(cols.saturating_sub(1));
            }
            ("up", _) | (_, Some('k')) => {
                let n = self.take_count() as u32;
                if self.row >= n {
                    self.row -= n;
                } else {
                    let overflow = (n - self.row) as isize;
                    self.row = 0;
                    let _ = term.scroll_delta(-overflow);
                }
            }
            ("down", _) | (_, Some('j')) => {
                let n = self.take_count() as u32;
                let max_row = rows.saturating_sub(1) as u32;
                if self.row + n <= max_row {
                    self.row += n;
                } else {
                    let overflow = (self.row + n - max_row) as isize;
                    self.row = max_row;
                    let _ = term.scroll_delta(overflow);
                }
            }
            ("pageup", _) => {
                let _ = term.scroll_delta(-(rows as isize));
            }
            ("pagedown", _) => {
                let _ = term.scroll_delta(rows as isize);
            }
            ("home", _) | (_, Some('0')) => self.col = 0,
            (_, Some('^')) => {
                let line = term.viewport_row_text(self.row);
                self.col = line.chars().position(|c| !c.is_whitespace()).unwrap_or(0) as u16;
            }
            ("end", _) | (_, Some('$')) => {
                let line = term.viewport_row_text(self.row);
                self.col = line.chars().count().saturating_sub(1).min(cols.saturating_sub(1) as usize) as u16;
            }
            (_, Some('H')) => self.row = 0,
            (_, Some('M')) => self.row = (rows / 2) as u32,
            (_, Some('L')) => self.row = rows.saturating_sub(1) as u32,
            (_, Some('g')) => {
                self.pending = Some('g');
                return CopyModeAction::Continue;
            }
            (_, Some('G')) => {
                let _ = term.scroll_to_bottom();
                self.row = rows.saturating_sub(1) as u32;
            }
            (_, Some('w')) => {
                let n = self.take_count();
                for _ in 0..n {
                    self.word_motion(term, true, false, false);
                }
            }
            (_, Some('W')) => {
                let n = self.take_count();
                for _ in 0..n {
                    self.word_motion(term, true, true, false);
                }
            }
            (_, Some('e')) => {
                let n = self.take_count();
                for _ in 0..n {
                    self.word_motion(term, true, false, true);
                }
            }
            (_, Some('E')) => {
                let n = self.take_count();
                for _ in 0..n {
                    self.word_motion(term, true, true, true);
                }
            }
            (_, Some('b')) => {
                let n = self.take_count();
                for _ in 0..n {
                    self.word_motion(term, false, false, false);
                }
            }
            (_, Some('B')) => {
                let n = self.take_count();
                for _ in 0..n {
                    self.word_motion(term, false, true, false);
                }
            }
            (_, Some('{')) => {
                let n = self.take_count();
                for _ in 0..n {
                    self.paragraph_motion(term, false);
                }
            }
            (_, Some('}')) => {
                let n = self.take_count();
                for _ in 0..n {
                    self.paragraph_motion(term, true);
                }
            }
            (_, Some(op @ ('f' | 'F' | 't' | 'T'))) => {
                self.pending = Some(op);
                return CopyModeAction::Continue;
            }
            (_, Some(';')) => {
                if let Some((kind, c)) = self.last_char_find {
                    let n = self.take_count();
                    for _ in 0..n {
                        self.find_char_in_line(term, kind, c);
                    }
                }
            }
            (_, Some(',')) => {
                if let Some((kind, c)) = self.last_char_find {
                    let rev = match kind {
                        CharFindKind::Forward => CharFindKind::Backward,
                        CharFindKind::Backward => CharFindKind::Forward,
                        CharFindKind::TillForward => CharFindKind::TillBackward,
                        CharFindKind::TillBackward => CharFindKind::TillForward,
                    };
                    let n = self.take_count();
                    for _ in 0..n {
                        self.find_char_in_line(term, rev, c);
                    }
                }
            }
            (_, Some('/')) => {
                self.search_prompt = Some((true, String::new()));
                return CopyModeAction::Continue;
            }
            (_, Some('?')) => {
                self.search_prompt = Some((false, String::new()));
                return CopyModeAction::Continue;
            }
            (_, Some('n')) => {
                if let Some((fwd, q)) = self.last_search.clone() {
                    self.jump_search(term, fwd, &q);
                }
            }
            (_, Some('N')) => {
                if let Some((fwd, q)) = self.last_search.clone() {
                    self.jump_search(term, !fwd, &q);
                }
            }
            _ => {
                let _ = shift;
                return CopyModeAction::Continue;
            }
        }

        self.sync_selection(term);
        CopyModeAction::Continue
    }

    fn toggle_visual(&mut self, term: &GhosttyTerminal, kind: VisualKind) {
        if self.visual == Some(kind) {
            self.visual = None;
            self.anchor = None;
        } else {
            if self.anchor.is_none() {
                self.anchor = Some((self.col, self.abs_row(term)));
            }
            self.visual = Some(kind);
        }
    }

    fn sync_selection(&self, term: &mut GhosttyTerminal) {
        let Some(kind) = self.visual else {
            let _ = term.clear_selection();
            return;
        };
        let Some((ac, ar)) = self.anchor else {
            let _ = term.clear_selection();
            return;
        };
        let (cols, rows) = term.size();
        let sb = term.scrollbar();
        let start_row = ar.saturating_sub(sb.offset).min(rows.saturating_sub(1) as u64) as u32;
        match kind {
            VisualKind::Char => {
                let _ = term.set_viewport_selection((ac, start_row), (self.col, self.row), false);
            }
            VisualKind::Line => {
                let (r0, r1) = if start_row <= self.row { (start_row, self.row) } else { (self.row, start_row) };
                let _ = term.set_viewport_selection((0, r0), (cols.saturating_sub(1), r1), false);
            }
            VisualKind::Block => {
                let _ = term.set_viewport_selection((ac, start_row), (self.col, self.row), true);
            }
        }
    }

    fn word_motion(&mut self, term: &GhosttyTerminal, forward: bool, big_word: bool, end: bool) {
        let line = term.viewport_row_text(self.row);
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            return;
        }
        let class = |c: char| -> u8 {
            if c.is_whitespace() {
                0
            } else if big_word || c.is_alphanumeric() || c == '_' {
                1
            } else {
                2
            }
        };
        let mut i = (self.col as usize).min(chars.len().saturating_sub(1));
        if forward {
            if end {
                if i + 1 < chars.len() {
                    i += 1;
                    while i < chars.len() && class(chars[i]) == 0 {
                        i += 1;
                    }
                    if i < chars.len() {
                        let cls = class(chars[i]);
                        while i + 1 < chars.len() && class(chars[i + 1]) == cls {
                            i += 1;
                        }
                    }
                }
            } else {
                let cls = class(chars[i]);
                while i < chars.len() && class(chars[i]) == cls {
                    i += 1;
                }
                while i < chars.len() && class(chars[i]) == 0 {
                    i += 1;
                }
            }
            self.col = i.min(chars.len().saturating_sub(1)) as u16;
        } else if i > 0 {
            i -= 1;
            while i > 0 && class(chars[i]) == 0 {
                i -= 1;
            }
            let cls = class(chars[i]);
            while i > 0 && class(chars[i - 1]) == cls {
                i -= 1;
            }
            self.col = i as u16;
        }
    }

    fn paragraph_motion(&mut self, term: &mut GhosttyTerminal, down: bool) {
        let (_, rows) = term.size();
        let mut r = self.row;
        let step = |r: u32| if down { r.checked_add(1).filter(|&x| x < rows as u32) } else { r.checked_sub(1) };
        while let Some(nr) = step(r) {
            r = nr;
            if term.viewport_row_text(r).trim().is_empty() {
                break;
            }
        }
        self.row = r;
        self.col = 0;
    }

    fn find_char_in_line(&mut self, term: &GhosttyTerminal, kind: CharFindKind, target: char) {
        let line = term.viewport_row_text(self.row);
        let chars: Vec<char> = line.chars().collect();
        let cur = self.col as usize;
        match kind {
            CharFindKind::Forward | CharFindKind::TillForward => {
                if let Some(rel) = chars.iter().skip(cur + 1).position(|&c| c == target) {
                    let idx = cur + 1 + rel;
                    self.col = if kind == CharFindKind::TillForward { idx.saturating_sub(1) } else { idx } as u16;
                }
            }
            CharFindKind::Backward | CharFindKind::TillBackward => {
                if let Some(idx) = chars.iter().take(cur).rposition(|&c| c == target) {
                    self.col = if kind == CharFindKind::TillBackward { (idx + 1).min(chars.len().saturating_sub(1)) } else { idx } as u16;
                }
            }
        }
    }

    fn jump_search(&mut self, term: &GhosttyTerminal, forward: bool, query: &str) {
        let needle = query.to_ascii_lowercase();
        let (_, rows) = term.size();
        let total = rows as usize;
        for step in 0..total {
            let r = if forward { (self.row as usize + step) % total } else { (self.row as usize + total - step) % total };
            let line = term.viewport_row_text(r as u32);
            let lower = line.to_ascii_lowercase();
            let start = if step == 0 && forward { (self.col as usize + 1).min(lower.len()) } else { 0 };
            if let Some(pos) = lower[start..].find(&needle) {
                self.row = r as u32;
                self.col = (start + pos) as u16;
                return;
            }
        }
    }
}
