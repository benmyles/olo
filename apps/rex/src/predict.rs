//! Typed letters shown at once, over a link that takes its time: what
//! mosh does for a terminal, kept to what can be made sure of, and made
//! to work in the programs people live in now (Ben, 2026-11-22: "can
//! take a second or two to see a letter appear. i know mosh was a
//! thing. can we build something like that into rex terminal but make
//! it work better?"; "it is critical that it work very well with
//! complex TUIs like claude code / codex / agy not just plain terminal
//! commands").
//!
//! A letter typed into a terminal on a far host shows when the host's
//! echo of it is back: a round trip and a half of the link as Rex types
//! (HACKING, Rex, "A slow link, measured"), 250 to 450 ms to a host 200
//! ms away, far more when a packet is lost. Here the letter is drawn
//! where the caret is as it is typed, and the host's echo, when it
//! comes, is checked against it and takes its place. Nothing of this is
//! sent anywhere: it is drawing.
//!
//! **The caret** is the terminal's cursor where it is shown (a shell,
//! an editor, Codex, agy, Claude Code as it runs now), and otherwise
//! the one cell a program draws in reverse video as its own (programs
//! built on Ink hide the cursor and draw theirs, as Claude Code did):
//! [`Screen`] is the caret's row and where on it the caret is,
//! whichever it is.
//!
//! **What is predicted**: a letter typed (one cell wide, or two), which
//! goes in at the caret; Backspace, and the left arrow, over what was
//! typed in the same run; the right arrow and Delete over what the
//! caret was moved back across. What is right of the caret is pushed
//! along where it is the line's own text, written over where it is
//! drawn dimmer (a shell's suggestion, a program's placeholder), and
//! never touched past a gap of two cells, a box's edge, a change of
//! ground or the last column (a right prompt, a pane's border, an input
//! box's own edge). Everything else (Enter, Tab, history, a control
//! key, a paste, a letter that would wrap) is not: the host says what
//! it does.
//!
//! **When it is shown** (a prediction made is not a prediction shown).
//! Mosh's rule, strictly: what is typed is unseen until a letter typed
//! in the same *epoch* has come back from the host as predicted. A new
//! epoch begins at every key that isn't predicted, at every prediction
//! that proved wrong, and whenever the caret's place or the text before
//! it changes by the host's doing alone (a prompt that appears by
//! itself: the first cut kept its trust through that, and would have
//! drawn a password asked for without an Enter of ours; found by
//! Gemini's review, 2026-11-22). So a password, or a program that
//! doesn't echo what is typed, never shows a letter: none of its epoch
//! is ever echoed. More, for the prompts that echo a mask: a letter the
//! host draws as something else taints its line (nothing of it is shown
//! or believed again while the caret stays on it), and a mask's own
//! letters (`*`, `•`…) prove nothing.
//! And only when the echo is slow enough to see ([`SHOW_AT`]): on this
//! computer or a near one the host's own echo is the picture.
//!
//! **How it is checked**. What was sent and isn't settled waits in a
//! queue: keys that are predicted, and *gaps* (what isn't). The keys at
//! its head are *placed*: applied one by one to the last screen the
//! host showed, they give the screens it may show on its way, and a
//! frame that is one of them confirms the keys up to it (frames skip).
//! Rows are known by what they hold, not where they are: an input box
//! moves as what is above it grows. Keys behind a gap aren't placed
//! (where the gap leaves the caret is the host's to say): they are
//! looked for in each frame as a change from the frame before it (the
//! same letters, appearing at the caret), which is also how a gap is
//! known to be over. A frame that is none of the screens expected is
//! not believed at once: a program's redraw comes in pieces, so it is
//! given [`SETTLE`] to become one; then what was drawn is given up, and
//! its keys go on being looked for as a gap's are. Keys the host has
//! taken (their write answered) and not echoed within [`NO_ECHO`] are
//! given up too.
//!
//! This is the client's half, and works with any host. The host's half
//! (a helper that says, in the stream of a terminal's output, what it
//! has taken and what state the terminal is in) is to make the first
//! letter at a shell's prompt immediate too: see TODO, "A terminal that
//! answers at once".

use std::collections::VecDeque;
use std::time::{Duration, Instant};

mod pane;
pub use pane::{predicted_row, screen_of};

/// The echo's delay from which typed letters are shown ahead of it:
/// about three frames at 60 Hz. Under it the host's own echo is as
/// quick as the eye.
pub const SHOW_AT: Duration = Duration::from_millis(45);

/// How long after the host has taken a key (its write answered) its
/// echo is waited for, at least, before it is given up: the echo
/// travels another way than the answer and may trail it, and a busy
/// program reads late.
pub const NO_ECHO: Duration = Duration::from_millis(400);

/// How long a frame that is none of the screens expected is waited on
/// before it is believed: a program that redraws its screen sends it in
/// pieces, and a piece is not a contradiction.
pub const SETTLE: Duration = Duration::from_millis(24);

/// The longest an echo is waited for after the host has taken its key,
/// however slow the link.
const MOST_WAIT: Duration = Duration::from_millis(2500);

/// How long before a frame a key must have been taken for the frame to
/// have held its echo, if the program echoed: a program reads what is
/// there for it in one go.
const BURST: Duration = Duration::from_millis(150);

/// How long a key behind a gap is looked for after the host has taken
/// it.
const ADRIFT: Duration = Duration::from_millis(200);

/// How long the host is silent about something typed before that is
/// said (in the window's status bar), at least.
pub const STALLED: Duration = Duration::from_secs(2);

/// The most keys of one write that are predicted: typing, a key held
/// down, a word from an input method. More at once is a paste, which
/// the host shows.
const MOST_AT_ONCE: usize = 8;

/// The most kept waiting: past it (a link that has stopped) what is
/// typed is no longer followed.
const MOST_WAITING: usize = 192;

/// The second cell of a letter two cells wide.
pub const WIDE_TAIL: char = '\u{FFFE}';

/// A cell that holds more than one plain letter (a cluster): equal to
/// nothing predicted.
pub const OTHER: char = '\u{FFFF}';

/// One cell of the caret's row, as far as predicting needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// Its letter: a space where it is empty, [`WIDE_TAIL`], [`OTHER`].
    pub ch: char,
    /// Drawn dimmer than text: a shell's suggestion, a program's
    /// placeholder. Not text of the line.
    pub dim: bool,
    /// On a ground of its own (a background, or reverse video).
    pub ground: bool,
}

impl Cell {
    pub const BLANK: Cell = Cell { ch: ' ', dim: false, ground: false };

    pub fn of(ch: char) -> Cell {
        Cell { ch, ..Cell::BLANK }
    }
}

/// What of a frame a prediction is made on and checked against: the
/// caret's row and where on it the caret is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    /// Where the row is in the pane (to draw on: rows are known by
    /// what they hold, never by this).
    pub row: u16,
    pub col: u16,
    pub cells: Vec<Cell>,
    /// The caret is one the program draws (the terminal's cursor is
    /// hidden): the cell at `col` is it.
    pub drawn: bool,
    /// The cursor's shape, as a number: a change of it is a change of
    /// mode (an editor's, a shell's vi keys).
    pub shape: u8,
    /// The alternate screen.
    pub alt: bool,
}

impl Screen {
    /// The text before the caret.
    fn before(&self) -> impl Iterator<Item = char> + '_ {
        self.cells[..(self.col as usize).min(self.cells.len())].iter().map(|c| c.ch)
    }

    /// Whether the caret is at the same place of the same line, in the
    /// same mode: nothing has happened that typing must be proven
    /// against anew. (Not where the row is: that moves.)
    fn same_place(&self, other: &Screen) -> bool {
        self.col == other.col && self.drawn == other.drawn && self.shape == other.shape && self.alt == other.alt && self.cells.len() == other.cells.len() && self.before().eq(other.before())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Type(char),
    Wide(char),
    Erase,
    Left,
    Right,
    Delete,
}

/// The keys a write is, where all of it is predicted.
fn keys_of(bytes: &[u8]) -> Option<Vec<Key>> {
    match bytes {
        b"\x1b[D" | b"\x1bOD" => return Some(vec![Key::Left]),
        b"\x1b[C" | b"\x1bOC" => return Some(vec![Key::Right]),
        b"\x1b[3~" => return Some(vec![Key::Delete]),
        _ => {}
    }
    if bytes.is_empty() || bytes.len() > MOST_AT_ONCE * 4 {
        return None;
    }
    let mut keys = Vec::new();
    for c in std::str::from_utf8(bytes).ok()?.chars() {
        if keys.len() == MOST_AT_ONCE {
            return None;
        }
        keys.push(match c {
            '\x7f' | '\x08' => Key::Erase,
            c if c.is_control() => return None,
            c if c.is_ascii() => Key::Type(c),
            c => match olo_rex_vt::codepoint_width(c) {
                1 => Key::Type(c),
                2 => Key::Wide(c),
                _ => return None,
            },
        });
    }
    Some(keys)
}

/// Whether a letter seen echoed says the host echoes what is typed: a
/// space, and what a prompt for a secret draws for every letter, don't.
fn telling(c: char) -> bool {
    !(c == ' ' || matches!(c, '*' | '•' | '●' | '·' | '∙' | '⋅' | '◦' | '○' | '▪' | '■' | '█' | '▒' | '░'))
}

/// A cell that is an edge: of a box, a pane, a bar. Nothing typed moves
/// it or goes past it.
fn wall(c: char) -> bool {
    matches!(c, '\u{2500}'..='\u{259F}' | '\u{E0B0}'..='\u{E0D4}')
}

/// Where a cell of a predicted line is from: how it is to be drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum From {
    /// That cell of the row as the host drew it.
    Cell(u16),
    /// A letter typed, in that epoch.
    Typed(u64),
    Blank,
}

/// The caret's row with keys applied to it.
#[derive(Clone, Debug)]
struct Line {
    cells: Vec<(Cell, From)>,
    col: u16,
    /// The leftmost place the caret may be taken back to: where this
    /// run of typing began. (What is before it is a prompt's, or text
    /// this client didn't see typed.)
    floor: u16,
    /// How many cells right of the caret are known to be the line's
    /// own text: the caret was moved back across them.
    right: u16,
    drawn: bool,
    /// The cells touched: from the lowest to past the highest.
    low: u16,
    high: u16,
}

impl Line {
    fn of(screen: &Screen, floor: u16, right: u16) -> Line {
        Line { cells: screen.cells.iter().enumerate().map(|(i, c)| (*c, From::Cell(i as u16))).collect(), col: screen.col, floor, right, drawn: screen.drawn, low: screen.col, high: screen.col }
    }

    fn blank(&self, i: usize) -> bool {
        self.cells.get(i).is_none_or(|c| c.0.ch == ' ')
    }

    /// The first cell from the caret that nothing typed may reach: an
    /// edge, a change of ground, the last column.
    fn stop(&self) -> usize {
        let last = self.cells.len().saturating_sub(1);
        let from = self.col as usize;
        // (The cell a program draws its caret in is on a ground of its
        // own: the ground is the next one's.)
        let like = if self.drawn { from + 1 } else { from };
        let ground = self.cells.get(like).is_some_and(|c| c.0.ground);
        (from..last).find(|&i| wall(self.cells[i].0.ch) || (i >= like && self.cells[i].0.ground != ground)).unwrap_or(last)
    }

    /// Where what is right of the caret ends: at a gap of two cells, or
    /// at `stop`. The caret's own place where there is nothing.
    fn end(&self, stop: usize) -> usize {
        let from = self.col as usize;
        (from..stop).find(|&i| self.blank(i) && (i + 1 >= stop || self.blank(i + 1))).unwrap_or(stop.max(from))
    }

    /// Whether what is at the caret is a hint (a suggestion, a
    /// placeholder), not text of the line: drawn dimmer. Under a caret
    /// the program draws, the cell itself is in reverse video and says
    /// nothing: the one after it does.
    fn hint(&self, col: usize) -> bool {
        self.cells.get(if self.drawn { col + 1 } else { col }).is_some_and(|c| c.0.dim)
    }

    fn touch(&mut self, from: usize, to: usize) {
        self.low = self.low.min(from as u16);
        self.high = self.high.max(to as u16);
    }

    /// `key` applied at the caret. None where it isn't one to predict
    /// here: the host says what it does.
    fn step(&mut self, key: Key, epoch: u64) -> Option<()> {
        let typed = |c: char| (Cell::of(c), From::Typed(epoch));
        let blank = (Cell::BLANK, From::Blank);
        let col = self.col as usize;
        if col >= self.cells.len() {
            return None;
        }
        let stop = self.stop();
        let end = self.end(stop);
        match key {
            Key::Type(_) | Key::Wide(_) => {
                let (c, n) = match key {
                    Key::Wide(c) => (c, 2),
                    Key::Type(c) => (c, 1),
                    _ => unreachable!(),
                };
                // (Never up to an edge: a program wraps before its own,
                // and what comes after the last column is a new line,
                // or isn't.)
                if col + n + 1 >= stop {
                    return None;
                }
                if end <= col {
                    self.touch(col, col + n);
                } else if self.hint(col) {
                    // A suggestion or a placeholder: written over. A
                    // shell's suggestion that goes on as typed stays;
                    // anything else goes (the program draws what it
                    // then has to say).
                    if n != 1 || self.drawn || self.cells[col].0.ch != c {
                        self.cells[col + n..end.max(col + n)].fill(blank);
                    }
                    self.touch(col, end.max(col + n));
                } else {
                    // The line's own text: pushed along.
                    if end + n >= stop {
                        return None;
                    }
                    self.cells.copy_within(col..end, col + n);
                    self.touch(col, end + n);
                }
                self.cells[col] = typed(c);
                if n == 2 {
                    self.cells[col + 1] = typed(WIDE_TAIL);
                }
                self.col += n as u16;
            }
            Key::Erase | Key::Left => {
                let n = if col >= 2 && self.cells[col - 1].0.ch == WIDE_TAIL { 2 } else { 1 };
                if col < self.floor as usize + n {
                    return None;
                }
                if key == Key::Left {
                    self.right += n as u16;
                } else if end > col && !self.hint(col) {
                    // The line's own text right of the caret: pulled back.
                    self.cells.copy_within(col..end, col - n);
                    self.cells[end - n..end].fill(blank);
                    self.touch(col - n, end);
                } else {
                    // (A suggestion goes with it: the shell draws another.)
                    self.cells[col - n..end.max(col)].fill(blank);
                    self.touch(col - n, end.max(col));
                }
                self.col -= n as u16;
                self.low = self.low.min(self.col);
            }
            Key::Right | Key::Delete => {
                let n = if self.cells.get(col + 1).is_some_and(|c| c.0.ch == WIDE_TAIL) { 2 } else { 1 };
                // (Only across what the caret was moved back over: what
                // else is right of it may be a suggestion, which the
                // right arrow takes.)
                if (self.right as usize) < n || end < col + n {
                    return None;
                }
                self.right -= n as u16;
                if key == Key::Right {
                    self.col += n as u16;
                    self.high = self.high.max(self.col);
                } else {
                    self.cells.copy_within(col + n..end, col);
                    self.cells[end - n..end].fill(blank);
                    self.touch(col, end);
                }
            }
        }
        Some(())
    }

    /// What `screen` proves of this line: the newest epoch of a letter
    /// typed that stands in it, before the caret, as typed. A letter
    /// typed and taken back proves nothing (the screen is as it was,
    /// which a host that echoes nothing also shows: a first cut took
    /// that for an echo, and a password typed after it would have been
    /// drawn; found by the played sessions, 2026-11-22); nor does a
    /// space, or what a secret's mask is drawn with.
    fn proof(&self, screen: &Screen) -> Option<u64> {
        let col = (self.col as usize).min(self.cells.len()).min(screen.cells.len());
        (0..col)
            .filter_map(|i| match self.cells[i] {
                (cell, From::Typed(epoch)) if telling(cell.ch) && cell.ch != WIDE_TAIL && screen.cells[i].ch == cell.ch && !screen.cells[i].dim => Some(epoch),
                _ => None,
            })
            .max()
    }

    /// Whether `screen` is this line: the caret where it is, and the
    /// text the keys touched (with two cells before it, to know the
    /// place by). What is right of the caret is the program's to draw
    /// (a suggestion, a hint): only what was moved there is looked at,
    /// and not where the program draws dimmer; nor the cell a program
    /// draws its caret in, where this line has nothing there (the
    /// first letter of a suggestion shows through it, in reverse and
    /// so not dim).
    ///
    /// (Where this line has a letter of its own under a drawn caret,
    /// text pushed along by what was typed before it, the screen must
    /// show it: a caret gone right over a letter leaves the same text
    /// before it as that letter typed again would, and only the cell
    /// under the caret tells them apart. The played sessions, each key
    /// sent as typed: a right arrow's frame passed for the echo of the
    /// "h" typed after it.)
    fn is(&self, screen: &Screen) -> bool {
        if screen.col != self.col || screen.cells.len() != self.cells.len() || screen.drawn != self.drawn {
            return false;
        }
        let from = (self.low as usize).saturating_sub(2);
        let col = self.col as usize;
        (from..col).all(|i| screen.cells[i].ch == self.cells[i].0.ch)
            && (col..(self.high as usize).min(self.cells.len())).all(|i| screen.cells[i].dim || screen.cells[i].ch == self.cells[i].0.ch || (self.drawn && i == col && self.cells[i].0.ch == ' '))
    }
}

/// What was sent and isn't settled.
#[derive(Clone, Copy, Debug)]
enum Sent {
    Key(Waiting),
    /// Something whose doing isn't predicted (the write it went in),
    /// or a prediction that proved wrong (no write): where it leaves
    /// the caret is the host's to say.
    Gap(u64),
}

/// Keys of the queue seen typed between two frames: which (`from` up
/// to `to`), and the epoch their letters on the screen prove.
#[derive(Clone, Copy, Debug)]
struct Found {
    from: usize,
    to: usize,
    proof: u64,
}

#[derive(Clone, Copy, Debug)]
struct Waiting {
    key: Key,
    /// The write it went in.
    seq: u64,
    at: Instant,
    /// When the host answered for it (took it), once it has.
    taken: Option<Instant>,
    epoch: u64,
}

/// How a cell of what is drawn ahead is to look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Like {
    /// As that cell of the row is drawn (text moved along).
    Cell(u16),
    /// As text typed there would be: like the letter before it.
    Typed,
    /// Empty.
    Blank,
    /// As the program draws its caret.
    Caret,
}

/// What is drawn over a frame: on `row` of the pane, from `col`, these
/// cells; and the caret at `caret` (the terminal's cursor is drawn
/// there; a caret the program draws is among the cells).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overlay {
    pub row: u16,
    pub col: u16,
    pub cells: Vec<(char, Like)>,
    pub caret: u16,
    /// Where the caret is in the frame itself.
    pub was: u16,
    pub drawn: bool,
}

/// One terminal's predictions.
#[derive(Debug)]
pub struct Predictor {
    on: bool,
    /// What was sent and isn't settled, oldest first. The keys before
    /// its first gap are placed on `base`; those behind one aren't.
    sent: VecDeque<Sent>,
    /// The last screen the host showed that the placed keys apply to
    /// (the last it showed at all, where none is placed).
    base: Option<Screen>,
    /// The last screen the host showed, whatever it was: what the next
    /// is a change from. And the one before it (what it was a change
    /// from, looked at again when a key's answer comes after its echo).
    last: Option<Screen>,
    last_at: Option<Instant>,
    prior: Option<Screen>,
    /// The change from `prior` to `last` was none of ours that could
    /// be told: the one change that is looked at again.
    unexplained: bool,
    /// A screen that is none of those expected, and since when: not
    /// believed until [`SETTLE`] has passed without a better one.
    doubt: Option<(Instant, Option<Screen>)>,
    floor: u16,
    right: u16,
    epoch: u64,
    /// The newest epoch one of whose letters was seen echoed.
    proven: u64,
    /// The writes that have left and aren't answered: up to which key,
    /// and when. And the quickest answers of late: an echo can't be
    /// back sooner than a write's answer is.
    went: VecDeque<(u64, Instant)>,
    trips: VecDeque<Duration>,
    /// The text before the caret of a line the host draws something
    /// else on for what is typed (a secret's mask): nothing of it is
    /// shown or believed while the caret's line begins so.
    taint: Option<Vec<char>>,
    /// The terminal is reading a secret, as the host said when last
    /// asked (its line discipline: no echo, a whole line at a time).
    secret: bool,
    /// Writes numbered as they are typed; the newest the host answered,
    /// and when.
    writes: u64,
    answered: u64,
    answered_at: Option<Instant>,
    /// How long an echo takes, of late (none seen yet: unknown).
    echo: Option<Duration>,
}

impl Default for Predictor {
    fn default() -> Self {
        Predictor::new(true)
    }
}

impl Predictor {
    pub fn new(on: bool) -> Predictor {
        Predictor {
            on,
            sent: VecDeque::new(),
            base: None,
            last: None,
            last_at: None,
            prior: None,
            unexplained: false,
            doubt: None,
            floor: 0,
            right: 0,
            epoch: 1,
            proven: 0,
            went: VecDeque::new(),
            trips: VecDeque::new(),
            taint: None,
            secret: false,
            writes: 0,
            answered: 0,
            answered_at: None,
            echo: None,
        }
    }

    /// Turned on or off (Preferences ▸ Rex): off, nothing waits and
    /// nothing is drawn; writes are still numbered.
    pub fn set_on(&mut self, on: bool) {
        if self.on != on {
            self.on = on;
            self.sent.clear();
            (self.base, self.last, self.prior, self.doubt) = (None, None, None, None);
            self.unexplained = false;
            self.epoch += 1;
        }
    }

    /// The host said whether the terminal reads a secret (asked of it
    /// at each pause in the terminal's output). Whether what is drawn
    /// changes.
    pub fn set_secret(&mut self, secret: bool) -> bool {
        let before = self.overlay();
        if secret && !self.secret {
            self.epoch += 1;
        }
        self.secret = secret;
        self.overlay() != before
    }

    /// A new epoch: what is typed from now is unseen until a letter of
    /// it comes true.
    fn anew(&mut self, screen: Option<&Screen>) {
        self.epoch += 1;
        self.floor = screen.map_or(0, |s| s.col);
        self.right = 0;
    }

    /// How many keys at the queue's head are placed.
    fn placed(&self) -> usize {
        self.sent.iter().take_while(|s| matches!(s, Sent::Key(_))).count()
    }

    fn keys(&self, from: usize, to: usize) -> impl Iterator<Item = &Waiting> {
        self.sent.range(from..to).filter_map(|s| match s {
            Sent::Key(w) => Some(w),
            Sent::Gap(_) => None,
        })
    }

    /// The base with the first `n` placed keys applied: where the
    /// keys can't be applied (the screen isn't what they were typed
    /// on), nothing.
    fn line(&self, n: usize) -> Option<Line> {
        let mut line = Line::of(self.base.as_ref()?, self.floor, self.right);
        for w in self.keys(0, n) {
            line.step(w.key, w.epoch)?;
        }
        Some(line)
    }

    /// A gap at the queue's head, and a new epoch: what was placed is
    /// placed no more.
    fn adrift(&mut self, screen: Option<Screen>) {
        if !matches!(self.sent.front(), Some(Sent::Gap(_)) | None) {
            self.sent.push_front(Sent::Gap(0));
        }
        if screen.is_some() {
            self.base = screen;
        }
        let base = self.base.clone();
        self.anew(base.as_ref());
        self.doubt = None;
    }

    /// `bytes` were typed and are being sent as one write: its number,
    /// to be given back when the host has answered it
    /// ([`Self::answered`]). `screen`: the terminal as last drawn, where
    /// it has a caret to type at. Whether what is drawn changes.
    pub fn typed(&mut self, bytes: &[u8], screen: Option<&Screen>, now: Instant) -> (u64, bool) {
        self.writes += 1;
        let seq = self.writes;
        if !self.on {
            return (seq, false);
        }
        let before = self.overlay();
        let keys = keys_of(bytes).filter(|keys| self.sent.len() + keys.len() < MOST_WAITING);
        match keys {
            Some(keys) => {
                if !self.follow(&keys, seq, screen, now) {
                    // Keys that are predicted, with nowhere to place them
                    // (no caret, or what waits before them can't be
                    // applied to the screen): looked for like those behind
                    // a gap, and unseen until one of them proves true.
                    if !matches!(self.sent.back(), Some(Sent::Gap(_))) {
                        self.sent.push_back(Sent::Gap(0));
                    }
                    self.epoch += 1;
                    self.float(&keys, seq, screen, now);
                }
            }
            None => {
                // Not one to predict: the host says what it does, and
                // what is typed after it is unseen until it proves true.
                if self.sent.len() >= MOST_WAITING {
                    self.sent.clear();
                }
                match self.sent.back_mut() {
                    Some(Sent::Gap(gap)) => *gap = seq,
                    _ => self.sent.push_back(Sent::Gap(seq)),
                }
                self.epoch += 1;
            }
        }
        (seq, self.overlay() != before)
    }

    fn follow(&mut self, keys: &[Key], seq: u64, screen: Option<&Screen>, now: Instant) -> bool {
        if self.placed() < self.sent.len() {
            self.float(keys, seq, screen, now);
            return true;
        }
        if self.sent.is_empty() {
            // Typed on what is drawn now. (Something else than was last
            // seen: the host's doing, to be proven against anew.)
            let Some(screen) = screen else { return false };
            if self.base.as_ref().is_none_or(|base| !base.same_place(screen)) {
                self.anew(Some(screen));
            }
            self.base = Some(screen.clone());
        }
        // On what is drawn with all that waits echoed.
        let Some(mut line) = self.line(self.sent.len()) else { return false };
        for key in keys {
            if line.step(*key, self.epoch).is_none() {
                return false;
            }
        }
        let epoch = self.epoch;
        self.sent.extend(keys.iter().map(|k| Sent::Key(Waiting { key: *k, seq, at: now, taken: None, epoch })));
        true
    }

    /// Keys behind a gap: not placed, only looked for (as a change from
    /// the screen before: this one, where frames weren't being looked
    /// at).
    fn float(&mut self, keys: &[Key], seq: u64, screen: Option<&Screen>, now: Instant) {
        if !self.waits() && screen.is_some() {
            self.last = screen.cloned();
            self.last_at = Some(now);
            self.prior = None;
            self.unexplained = false;
        }
        let epoch = self.epoch;
        self.sent.extend(keys.iter().map(|k| Sent::Key(Waiting { key: *k, seq, at: now, taken: None, epoch })));
    }

    /// What was typed up to write `seq` has left for the host, now.
    pub fn went(&mut self, seq: u64, now: Instant) {
        if self.went.back().is_none_or(|(last, _)| seq > *last) {
            if self.went.len() >= 32 {
                self.went.pop_front();
            }
            self.went.push_back((seq, now));
        }
    }

    /// Whether a key's echo can be back by now: the host has answered
    /// for it, or its write is the oldest the host hasn't answered and
    /// left at least most of a round trip ago (as quick a one as the
    /// link has shown of late). Where no round trip has been timed
    /// yet, only the answer says.
    ///
    /// (The oldest: where each key is sent as it is typed, several
    /// writes are on their way at once, and the host takes one only
    /// when it has answered the one before, which is what its helper
    /// there is for. The answer and the echo come back over one
    /// connection, in the order they were sent, so no echo of a write
    /// is back before the answer to the write before it. The played
    /// sessions found the need: of two Enters and an "x" on their way
    /// behind an "x" typed before them, the first x's echo passed for
    /// the second's.)
    fn can_be_back(&self, seq: u64, now: Instant) -> bool {
        if seq <= self.answered {
            return true;
        }
        let Some(quickest) = self.trips.iter().min() else { return false };
        self.went.front().filter(|(upto, _)| seq <= *upto).is_some_and(|(_, at)| now.saturating_duration_since(*at) >= *quickest * 7 / 10)
    }

    /// The host answered write `seq` (it has taken what was typed up to
    /// there). Whether what is drawn changes: a key is only held to
    /// have been seen echoed when its echo can be back, which a first
    /// answer can be the first to say, so the last frame is looked at
    /// again.
    pub fn answered(&mut self, seq: u64, now: Instant) -> bool {
        if seq <= self.answered {
            return false;
        }
        self.answered = seq;
        self.answered_at = Some(now);
        for sent in self.sent.iter_mut() {
            if let Sent::Key(w) = sent
                && w.seq <= seq
                && w.taken.is_none()
            {
                w.taken = Some(now);
            }
        }
        while let Some((upto, at)) = self.went.front().copied() {
            if upto > seq {
                break;
            }
            self.went.pop_front();
            if upto == seq {
                // (The quickest of the last few: a link's delay changes.)
                if self.trips.len() >= 8 {
                    self.trips.pop_front();
                }
                self.trips.push_back(now.saturating_duration_since(at));
            }
        }
        // (Only a change nothing was made of when it came: one a key was
        // seen in is that key's, however like another's it looks.)
        if !self.on || !self.unexplained {
            return false;
        }
        let before = self.overlay();
        if let (Some(prior), Some(last)) = (self.prior.clone(), self.last.clone())
            && let Some(found) = self.found(&prior, &last, now)
        {
            self.last = Some(prior);
            self.seen(found, &last, now);
            self.last = Some(last);
            self.unexplained = false;
        }
        self.overlay() != before
    }

    /// A frame of the terminal: `screen` as [`Self::typed`] takes it
    /// (none: no caret is to be found in it). Not to be called in the
    /// middle of a program's update (its synchronized output): a piece
    /// of a frame is not a frame. Whether what is drawn changes.
    pub fn frame(&mut self, screen: Option<&Screen>, now: Instant) -> bool {
        if !self.on {
            return false;
        }
        let before = self.overlay();
        if let (Some(taint), Some(screen)) = (&self.taint, screen)
            && !(screen.before().count() >= taint.len() && screen.before().zip(taint.iter()).all(|(a, b)| a == *b))
        {
            self.taint = None;
        }
        let placed = self.placed();
        let mut explained = true;
        if placed > 0 {
            if let Some((screen, (n, line))) = screen.and_then(|screen| self.place(screen, placed).map(|at| (screen, at))) {
                self.confirm(0, n, line.proof(screen), now);
                // (What is known of the line goes with the keys seen.)
                (self.floor, self.right) = (line.floor, line.right);
                self.sent.drain(..n);
                self.base = Some(screen.clone());
                self.doubt = None;
            } else if let Some((screen, line)) = screen.and_then(|screen| self.passed(screen, placed).map(|line| (screen, line))) {
                // All of them echoed, and what was typed after them (not
                // predicted) already at work: they are seen, it isn't over.
                self.confirm(0, placed, line.proof(screen), now);
                self.sent.drain(..placed);
                self.base = Some(screen.clone());
                self.doubt = None;
            } else if let Some((screen, found)) = screen.and_then(|screen| self.last.as_ref().and_then(|last| self.found(last, screen, now)).map(|found| (screen, found))) {
                // Not where they were placed (the line moved), but seen
                // typed between the last frame and this.
                self.seen(found, screen, now);
            } else {
                // None of the screens expected. A piece of a redraw,
                // or the prediction was wrong: time says which.
                let since = self.doubt.as_ref().map_or(now, |d| d.0);
                self.doubt = Some((since, screen.cloned()));
                explained = false;
            }
        } else if let Some(screen) = screen {
            if self.sent.is_empty() {
                if self.base.as_ref().is_none_or(|base| !base.same_place(screen)) {
                    self.anew(Some(screen));
                }
                self.base = Some(screen.clone());
            } else if let Some(found) = self.last.as_ref().and_then(|last| self.found(last, screen, now)) {
                // Keys behind the gap, seen typed between the last frame
                // and this: the gap is over, and what came before them
                // was taken.
                self.seen(found, screen, now);
            } else {
                self.base = Some(screen.clone());
                explained = false;
            }
        } else if self.sent.is_empty() && self.base.take().is_some() {
            // (No caret: a program has the screen to itself.)
            self.anew(None);
        }
        if screen.is_some() {
            self.prior = std::mem::replace(&mut self.last, screen.cloned());
            self.last_at = Some(now);
            self.unexplained = !explained;
        }
        self.overlay() != before
    }

    /// Which of the screens on the way from the base `screen` is: how
    /// many of the placed keys it shows echoed, and the line they make.
    /// The nearest that fits. Two can look alike (a letter typed and
    /// taken back leaves the line as it was): taken for the further at
    /// a guess, the letter came back a moment later and was seen,
    /// drawn, then gone. (Keys that come to nothing are let go when
    /// their time is up: [`Self::tick`].)
    fn place(&self, screen: &Screen, placed: usize) -> Option<(usize, Line)> {
        let mut line = Line::of(self.base.as_ref()?, self.floor, self.right);
        if line.is(screen) {
            return Some((0, line));
        }
        for (i, w) in self.keys(0, placed).enumerate() {
            line.step(w.key, w.epoch)?;
            if line.is(screen) {
                return Some((i + 1, line));
            }
        }
        None
    }

    /// Whether `screen` is past all the placed keys (the line they
    /// make, if so): it shows them echoed as predicted and the caret
    /// gone on from there, and what was typed after them is something
    /// not predicted (a completion that added to them, say). So they
    /// came true, though no frame showed them alone.
    fn passed(&self, screen: &Screen, placed: usize) -> Option<Line> {
        if !matches!(self.sent.get(placed), Some(Sent::Gap(_))) {
            return None;
        }
        let line = self.line(placed)?;
        let from = (line.low as usize).saturating_sub(2);
        (screen.col > line.col && screen.cells.len() == line.cells.len() && screen.drawn == line.drawn && (from..line.col as usize).all(|i| screen.cells[i].ch == line.cells[i].0.ch)).then_some(line)
    }

    /// Keys were seen typed between the last frame and `screen`: they,
    /// and all of the queue before them, are over.
    fn seen(&mut self, found: Found, screen: &Screen, now: Instant) {
        self.confirm(found.from, found.to, Some(found.proof), now);
        self.sent.drain(..found.to);
        self.floor = self.last.as_ref().map_or(screen.col, |last| last.col.min(screen.col));
        self.right = 0;
        self.base = Some(screen.clone());
        self.doubt = None;
    }

    /// Keys of the queue seen typed between `last` and `screen`: the
    /// same letters, appearing at the caret. How a gap is known to be
    /// over, and how keys are found again when the line they were
    /// placed on has moved. Only a run that leaves a letter to be seen
    /// (a letter typed and taken back fits a screen that didn't change,
    /// which proves nothing).
    ///
    /// The same letter is typed more than once, and a frame's one new
    /// letter fits each of them. Which it is, is said by what can be
    /// known; where that doesn't say, nothing is held to be seen (the
    /// next letter typed will tell). Each rule here closes something
    /// the played sessions found:
    /// - Only keys whose echo can be back ([`Self::can_be_back`]). What
    ///   isn't predicted can look like a key that is (a completion that
    ///   adds the letter typed next): the key typed after it has only
    ///   just left when that shows.
    /// - Keys may have gone unseen (echoed in a frame with other
    ///   things), so a run may begin after some of its stretch (the
    ///   keys between two gaps); but only where what those typed stands
    ///   before the caret in `last`: they are passed over as echoed,
    ///   and are seen to be. (Else, of two "x", the second still on its
    ///   way was held to be the one seen.) A run that so stands on what
    ///   is seen is taken before one that begins a stretch, of which
    ///   nothing says what came before it: the first "l" of "ll",
    ///   echoed with the prompt, isn't the one seen in the second's
    ///   frame; and of "s", Tab, "s", the first is the one seen first.
    /// - Not a run the host would have gone on from: the key after it
    ///   was taken long before `last` was shown, and isn't in this
    ///   frame with it. A program that echoes reads what is there in
    ///   one go; so this is something else that looks like the echo.
    ///   (Else a letter typed long before into a line that echoed
    ///   nothing was held to be the one seen, and the rest of that line,
    ///   a password, was drawn after it.)
    /// - Not from before a later stretch whose letters stand on the
    ///   screen: what was sent before those was taken before them.
    fn found(&self, last: &Screen, screen: &Screen, now: Instant) -> Option<Found> {
        if last.cells.len() != screen.cells.len() {
            return None;
        }
        let before: Vec<char> = last.before().collect();
        // The runs that fit: those that stand on what is seen of their
        // stretch, and those that begin one.
        let (mut standing, mut beginning): (Vec<Found>, Vec<Found>) = (Vec::new(), Vec::new());
        // The keys of a stretch so far: what they typed since the caret
        // was last moved; where the stretch begins; and the latest
        // stretch whose letters are on the screen.
        let mut typed: Vec<char> = Vec::new();
        let (mut stretch, mut alive) = (0, 0);
        for from in 0..self.sent.len() {
            let Sent::Key(first) = self.sent[from] else {
                typed.clear();
                stretch = from + 1;
                continue;
            };
            if before.ends_with(&typed) {
                let mut line = Line::of(last, last.col, 0);
                for to in from..self.sent.len() {
                    let Sent::Key(w) = self.sent[to] else { break };
                    if !self.can_be_back(w.seq, now) || line.step(w.key, w.epoch).is_none() {
                        break;
                    }
                    let gone_on = matches!(self.sent.get(to + 1), Some(Sent::Key(Waiting { key: Key::Type(_) | Key::Wide(_), taken: Some(taken), .. })) if self.last_at.is_some_and(|shown| *taken + BURST < shown));
                    if !gone_on
                        && line.is(screen)
                        && let Some(proof) = line.proof(screen)
                    {
                        // (The nearest that fits, as for keys that are placed.)
                        // (Standing on a letter that says something: a
                        // prompt ends in a space too.)
                        let found = Found { from, to: to + 1, proof };
                        if typed.iter().any(|c| telling(*c)) {
                            standing.push(found)
                        } else {
                            beginning.push(found)
                        }
                        break;
                    }
                }
            }
            match first.key {
                Key::Type(c) => typed.push(c),
                Key::Wide(c) => typed.extend([c, WIDE_TAIL]),
                Key::Erase => {
                    typed.pop();
                }
                // (The caret moved: what was typed before it is no
                // longer all before it.)
                Key::Left | Key::Right => typed.clear(),
                Key::Delete => {}
            }
            if typed.last().is_some_and(|c| telling(*c)) && before.ends_with(&typed) && self.can_be_back(first.seq, now) {
                alive = stretch;
            }
        }
        let fits = if standing.is_empty() { beginning } else { standing };
        match fits.as_slice() {
            [found] if found.from >= alive => Some(*found),
            _ => None,
        }
    }

    /// The keys of the queue from `from` to `to` were seen echoed: how
    /// long the echo took; and `proof`, the epoch a letter of theirs
    /// standing on the screen proves.
    fn confirm(&mut self, from: usize, to: usize, proof: Option<u64>, now: Instant) {
        let mut echo = self.echo;
        for w in self.keys(from, to) {
            if matches!(w.key, Key::Type(_) | Key::Wide(_)) {
                let took = now.saturating_duration_since(w.at);
                // (Of late: a quarter of each new one.)
                echo = Some(echo.map_or(took, |echo| (echo * 3 + took) / 4));
            }
        }
        self.echo = echo;
        if self.taint.is_none() {
            self.proven = self.proven.max(proof.unwrap_or(0));
        }
    }

    /// What time brings: a frame that was none of those expected is
    /// believed after [`SETTLE`], and keys the host took and didn't
    /// echo are given up. When to ask again, if anything waits on
    /// time; and whether what is drawn changed.
    pub fn tick(&mut self, now: Instant) -> (Option<Instant>, bool) {
        if !self.on {
            return (None, false);
        }
        let before = self.overlay();
        if let Some((since, screen)) = self.doubt.take() {
            if now >= since + SETTLE {
                self.wrong(screen);
            } else {
                self.doubt = Some((since, screen));
            }
        }
        // (A slow echo is waited for longer: twice what it takes of
        // late, within reason.)
        let wait = NO_ECHO.max(self.echo.unwrap_or_default() * 2).min(MOST_WAIT);
        let mut next = self.doubt.as_ref().map(|d| d.0 + SETTLE);
        // (Each from when the host took it.)
        let due = |w: &Waiting| w.taken.map(|taken| taken + wait);
        let over = self.keys(0, self.placed()).take_while(|w| due(w).is_some_and(|due| now >= due)).count();
        if over > 0 {
            // The host took keys that were placed and showed nothing
            // of them. Where they come to nothing (a letter typed
            // and taken back), that is what was predicted: over.
            // Else the prediction was wrong, and what is left is
            // placed no more.
            let came_to_nothing = self.line(over).filter(|line| self.base.as_ref().is_some_and(|base| line.is(base)));
            self.sent.drain(..over);
            match came_to_nothing {
                Some(line) => (self.floor, self.right) = (line.floor, line.right),
                None => self.adrift(None),
            }
        }
        // Behind a gap: only no longer looked for, and sooner. An echo
        // comes with its key's answer, give or take a moment, however
        // slow the link; a key the host has had for longer went into
        // something that doesn't echo, or reads late, and is not to be
        // taken for a later one's echo. (A password's letters, still
        // looked for at the next prompt, were: the played sessions.)
        let placed = self.placed();
        let mut at = 0;
        self.sent.retain(|s| {
            at += 1;
            !matches!(s, Sent::Key(w) if at > placed && w.taken.is_some_and(|taken| now >= taken + ADRIFT))
        });
        let placed = self.placed();
        let waits = self.sent.iter().enumerate().filter_map(|(i, s)| match s {
            Sent::Key(w) if i < placed => due(w),
            Sent::Key(w) => w.taken.map(|taken| taken + ADRIFT),
            Sent::Gap(_) => None,
        });
        next = next.into_iter().chain(waits).min();
        // A gap with nothing behind it is all that is left: it waits for
        // the next key typed, on no clock.
        (next, self.overlay() != before)
    }

    /// What was placed isn't what the host shows. Where it shows
    /// another letter for the one typed (the caret went on as
    /// predicted), the line draws a mask: tainted.
    fn wrong(&mut self, screen: Option<Screen>) {
        if let (Some(base), Some(screen)) = (&self.base, &screen)
            && let Some(Sent::Key(Waiting { key: Key::Type(typed), .. })) = self.sent.front()
            && screen.col > base.col
            && screen.cells.len() == base.cells.len()
            && screen.before().take(base.col as usize).eq(base.before())
            && screen.cells[base.col as usize].ch != *typed
            && screen.cells[base.col as usize].ch != ' '
        {
            self.taint = Some(base.before().collect());
        }
        let placed = self.placed();
        if self.taint.is_none() && matches!(self.sent.get(placed), Some(Sent::Gap(gap)) if *gap != 0 && *gap <= self.answered) {
            // What was typed after them (not predicted) has been taken
            // by the host, so they were before it: over, whatever it
            // made of the screen. Not a prediction proved wrong.
            self.sent.drain(..placed);
            if screen.is_some() {
                self.base = screen;
            }
            self.doubt = None;
            return;
        }
        self.adrift(screen);
    }

    /// What is drawn over the frame now: the placed keys that are to be
    /// seen (their epoch proven, the echo slow enough to see), on the
    /// screen they were typed on.
    pub fn overlay(&self) -> Option<Overlay> {
        if !self.on || self.secret || self.taint.is_some() || self.doubt.is_some() || self.echo.is_none_or(|echo| echo < SHOW_AT) {
            return None;
        }
        let base = self.base.as_ref()?;
        let seen = self.keys(0, self.placed()).take_while(|w| w.epoch <= self.proven).count();
        if seen == 0 {
            return None;
        }
        let line = self.line(seen)?;
        let (mut low, mut high) = (line.low.min(line.col) as usize, (line.high.max(line.col) as usize).min(line.cells.len()));
        if base.drawn {
            // (The caret the program draws: where it was is text again,
            // where it is now is drawn as it.)
            low = low.min(base.col as usize);
            high = high.max(base.col as usize + 1).max(line.col as usize + 1).min(line.cells.len());
        }
        let cells = (low..high)
            .map(|i| {
                let (cell, from) = line.cells[i];
                let like = match from {
                    _ if base.drawn && i == line.col as usize => Like::Caret,
                    From::Cell(c) if base.drawn && c == base.col => Like::Typed,
                    From::Cell(c) => Like::Cell(c),
                    From::Typed(_) => Like::Typed,
                    From::Blank => Like::Blank,
                };
                (cell.ch, like)
            })
            .collect();
        Some(Overlay { row: base.row, col: low as u16, cells, caret: line.col, was: base.col, drawn: base.drawn })
    }

    /// How long an echo takes of late, if one was seen.
    #[cfg(test)]
    pub fn echo(&self) -> Option<Duration> {
        self.echo
    }

    /// Whether keys wait to be seen echoed.
    pub fn waits(&self) -> bool {
        self.sent.iter().any(|s| matches!(s, Sent::Key(_)))
    }

    /// Whether frames are to be shown to it ([`Self::frame`]): keys wait
    /// to be seen echoed, or what is typed next would be shown at once
    /// (a trust the next frame may take away). A terminal nothing is
    /// typed into costs nothing: its frames aren't looked at.
    pub fn wants_frames(&self) -> bool {
        self.on && (self.waits() || self.doubt.is_some() || self.proven == self.epoch)
    }

    /// How long the host has been silent about something typed, where
    /// that is long enough to be said ([`STALLED`], or three of its
    /// quickest answers of late): it answers each write as it takes
    /// it, so the link has stopped, or the host has.
    pub fn stalled(&self, now: Instant) -> Option<Duration> {
        let (_, left) = self.went.front()?;
        let silent = now.saturating_duration_since(*left);
        (silent >= self.stall_after()).then_some(silent)
    }

    fn stall_after(&self) -> Duration {
        STALLED.max(self.trips.iter().min().copied().unwrap_or_default() * 3)
    }

    /// When what [`Self::stalled`] says next changes (it is said in
    /// whole seconds for a minute, in whole minutes from then), if
    /// something typed is unanswered.
    pub fn stall_changes(&self, now: Instant) -> Option<Instant> {
        let (_, left) = self.went.front()?;
        let silent = now.saturating_duration_since(*left);
        let after = self.stall_after();
        let secs = silent.as_secs();
        Some(
            *left
                + if silent < after {
                    after
                } else if secs < 60 {
                    Duration::from_secs(secs + 1)
                } else {
                    Duration::from_secs((secs / 60 + 1) * 60)
                },
        )
    }
}

#[cfg(test)]
mod played;
#[cfg(test)]
mod tests;
