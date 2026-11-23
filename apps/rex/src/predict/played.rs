//! Sessions played, by the thousand: a host, a link and someone
//! typing, every one different (Ben, 2026-11-22: "make sure there are
//! fuzz tests for this all this slow link stuff too so that we catch
//! and fix all the edge cases / race conditions").
//!
//! The host is a program that reads keys far away: a shell's line
//! editor (which may read a secret after an Enter, or draw a mask for
//! each letter, or offer a suggestion in grey), or a program that
//! draws its own caret in a box that moves and redraws in pieces (as
//! Claude Code does), and sometimes puts a question in the caret's
//! place. The link delays each way, stalls now and then, lets an echo
//! pass its answer, and a pane paints only some of the frames. Someone
//! else may type into the same line, and a prompt may change under it.
//!
//! Held, at every step of every session:
//! - nothing typed into a line that reads a secret or draws a mask is
//!   ever drawn;
//! - where nobody else types, what is drawn with a key in it is what
//!   the host then shows;
//! - when all is over, nothing is left drawn or waiting;
//! - and most of what is typed where it echoes is drawn at once (so a
//!   change that makes it too careful to be of use is seen).
//!
//! What these sessions found, as the model was written (the first three
//! would have drawn what was typed at a prompt that echoes nothing):
//! - a letter typed and taken back passing for an echo (the screen is
//!   as it was, which a host that echoes nothing also shows);
//! - a letter of a password, still looked for at the next prompt,
//!   passing for the echo of the same letter typed there;
//! - a prompt's last space counted as "what was typed before stands on
//!   the screen" for a key that had typed a space;
//! - the echo of a letter that couldn't be placed passing for the same
//!   letter typed after it;
//! - of two of the same letter, the one still on its way held to be
//!   the one seen; and the other way round ("ll" with its first "l"
//!   echoed in the prompt's frame; "s", Tab, "s");
//! - a frame's change, already a key's, looked at again at a late
//!   answer and taken for another's;
//! - a hint under a caret the program draws taken for the line's text.
//!
//! And when the host learnt to be asked about its prompt (it answers
//! here as a real one would, a shell reading a secret with its line
//! editor too, and someone reading what an Enter brought pauses), the
//! sessions were all new ones, and found seven more:
//! - a later stretch known to be on the screen only if all it typed
//!   stood before the caret, which someone else's letter in the line
//!   undid (a password's letter then passed for an echo);
//! - a letter typed and the caret taken back over it passing for
//!   "nothing yet", and for another key's frame (the cell under the
//!   caret wasn't looked at);
//! - a round trip timed for writes that had waited their turn behind
//!   others, so that "the quickest of late" was seconds;
//! - what a stretch typed "standing before the caret" in the very
//!   screen it was typed on, whose line happened to end with it;
//! - an echo's delay counted to the frame that told its key from
//!   another, long after;
//! - two runs that end with the same key held to be two;
//! - an old frame looked at again at a late answer, and taken for a key
//!   that had only just left when it was shown.
//!
//! `PLAY_ONLY=N` plays session N alone; with `PLAY_SAY=1` it says what
//! happens as it goes.

use std::collections::{HashMap, HashSet, VecDeque};

use super::*;

const WIDTH: usize = 48;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % below.max(1)
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next(100) < percent
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Echo,
    Silent,
    Mask,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// A shell's line editor: the terminal's cursor is the caret.
    Shell,
    /// A program that draws its caret itself, in a box.
    Boxed,
}

struct Host {
    kind: Kind,
    prompt: Vec<char>,
    text: Vec<char>,
    col: usize,
    mode: Mode,
    /// A suggestion in grey after the text (a shell's).
    suggestion: Vec<char>,
    /// A question in the caret's place: no caret, keys do nothing
    /// until Enter.
    asking: bool,
    /// A line that reads a secret is the shell's own, read with its
    /// line editor (bash's `read -es`): asked, the host says the shell
    /// is reading key by key, as at its prompt.
    by_the_shell: bool,
    row: u16,
}

impl Host {
    fn new(kind: Kind) -> Host {
        let prompt = match kind {
            Kind::Shell => "$ ",
            Kind::Boxed => "│ > ",
        };
        Host { kind, prompt: prompt.chars().collect(), text: Vec::new(), col: 0, mode: Mode::Echo, suggestion: Vec::new(), asking: false, by_the_shell: false, row: 3 }
    }

    /// A key, as the program takes it. `rng`: what the program does of
    /// its own accord at it (which kind of line follows an Enter, a
    /// suggestion offered, the box moved).
    fn key(&mut self, bytes: &[u8], rng: &mut Lcg) {
        if self.asking {
            self.asking = bytes != b"\r";
            return;
        }
        self.suggestion.clear();
        match bytes {
            b"\r" => {
                // (A command was given: only then does something else
                // than the prompt come to read the terminal.)
                let command = self.text.iter().any(|c| *c != ' ');
                self.text.clear();
                self.col = 0;
                self.row = (self.row + 1) % 20;
                match self.kind {
                    Kind::Shell => {
                        self.mode = match rng.next(10) {
                            0 | 1 => Mode::Silent,
                            2 => Mode::Mask,
                            _ => Mode::Echo,
                        };
                        self.by_the_shell = self.mode == Mode::Silent && command && rng.chance(50);
                        self.prompt = match self.mode {
                            Mode::Echo => "$ ",
                            Mode::Silent => "Password: ",
                            Mode::Mask => "PIN: ",
                        }
                        .chars()
                        .collect();
                    }
                    Kind::Boxed => self.asking = rng.chance(15),
                }
            }
            [0x7f] => {
                if self.col > 0 && self.mode != Mode::Silent {
                    self.text.remove(self.col - 1);
                    self.col -= 1;
                }
            }
            b"\x1b[D" => self.col -= usize::from(self.col > 0 && self.mode == Mode::Echo),
            b"\x1b[C" => {
                if self.mode == Mode::Echo && self.col < self.text.len() {
                    self.col += 1;
                }
            }
            [c] if self.prompt.len() + self.text.len() + 8 < WIDTH => match self.mode {
                Mode::Echo => {
                    self.text.insert(self.col, char::from(*c));
                    self.col += 1;
                    if self.kind == Kind::Shell && self.col == self.text.len() && rng.chance(20) {
                        self.suggestion = " --all".chars().collect();
                    }
                }
                Mode::Mask => {
                    self.text.push('*');
                    self.col += 1;
                }
                Mode::Silent => {}
            },
            _ => {}
        }
        if self.kind == Kind::Boxed && rng.chance(25) {
            self.row = (self.row + 1 + rng.next(3) as u16) % 20;
        }
    }

    fn screen(&self) -> Option<Screen> {
        if self.asking {
            return None;
        }
        let mut cells: Vec<Cell> = self.prompt.iter().chain(&self.text).map(|c| Cell::of(*c)).collect();
        let col = self.prompt.len() + self.col;
        let end = cells.len();
        cells.resize(WIDTH, Cell::BLANK);
        for (i, c) in self.suggestion.iter().enumerate() {
            cells[end + i] = Cell { ch: *c, dim: true, ground: false };
        }
        let drawn = self.kind == Kind::Boxed;
        if drawn {
            // Its hint where nothing is typed, its caret, its edge.
            if self.text.is_empty() {
                for (i, c) in "Type here".chars().enumerate() {
                    cells[end + i] = Cell { ch: c, dim: i > 0, ground: false };
                }
            }
            cells[col].ground = true;
            cells[WIDTH - 3] = Cell::of('│');
        }
        Some(Screen { row: self.row, col: col as u16, cells, drawn, shape: if drawn { u8::MAX } else { 0 }, alt: false })
    }

    /// What the host says when asked whether the shell itself is
    /// reading the terminal at its line editor: at its prompt; and at a
    /// secret it reads with that editor, where it is just as true.
    fn shell_reads(&self) -> bool {
        self.kind == Kind::Shell && !self.asking && (self.mode == Mode::Echo || (self.mode == Mode::Silent && self.by_the_shell))
    }
}

/// What a session is made of.
#[derive(Clone, Copy, Debug)]
struct Plan {
    seed: u64,
    kind: Kind,
    /// The link, each way, in ms.
    delay: u64,
    /// The link stalls now and then (a packet lost, and waited for).
    stalls: bool,
    /// The pane paints only some of the frames.
    drops: bool,
    /// Someone else types into the same line, and a shell's prompt
    /// changes under it.
    others: bool,
    /// Each key is sent as it is typed, several on their way at once
    /// (Olo's helper is on the host and gives them to the server one
    /// at a time, in order); not one write at a time with what waits
    /// gathered into the next.
    piped: bool,
}

#[derive(Default, Debug, Clone, Copy)]
struct Tally {
    /// Keys typed into a line that echoes them, and how many of those
    /// were drawn the moment they were typed.
    echoed: usize,
    at_once: usize,
    /// Of those, the first letter typed at a line, and how many of
    /// those were drawn at once (the host had said its shell reads at
    /// a prompt known to echo).
    firsts: usize,
    firsts_at_once: usize,
}

fn play(plan: Plan, keys: usize) -> Tally {
    #[derive(Debug)]
    enum Event {
        /// A write arrives at the host: its keys' numbers and bytes.
        Arrives(Vec<(u64, Vec<u8>)>),
        Answer(u64),
        /// A frame of the host's reaches the pane (none: no caret in it).
        Frame(Option<Screen>),
        /// Someone else's key, or the prompt changing.
        Other,
        Tick,
        /// The app's question (is the terminal at its shell's prompt?)
        /// reaches the host; and the host's answer the app.
        Asked(u64),
        Said(u64, bool),
    }
    let Plan { seed, delay, .. } = plan;
    let say = std::env::var("PLAY_SAY").is_ok();
    let text = |s: &Option<Screen>| s.as_ref().map_or("(no caret)".to_owned(), |s| format!("{:?}@{}", s.cells.iter().map(|c| c.ch).collect::<String>().trim_end(), s.col));
    let mut rng = Lcg(seed.wrapping_mul(0x9e37_79b9).wrapping_add(delay));
    let mut whim = Lcg(seed ^ 0x5bd1_e995);
    let t0 = Instant::now();
    let mut host = Host::new(plan.kind);
    let mut p = Predictor::new(true);
    let mut events: Vec<(u64, u64, Event)> = Vec::new();
    let mut order = 0u64;
    let mut push = |events: &mut Vec<(u64, u64, Event)>, at: u64, e: Event| {
        order += 1;
        events.push((at, order, e));
    };
    // What the pane last painted of the host.
    let mut seen = host.screen();
    p.frame(seen.as_ref(), t0);
    let mut outbox: VecDeque<(u64, Vec<u8>)> = VecDeque::new();
    let mut in_flight = false;
    // (What is on its way on one connection arrives in the order it
    // was sent, there and back: a packet held holds those behind it.)
    let (mut arrives_after, mut back_after) = (0u64, 0u64);
    // Each key by its write's number: with it drawn, the text before
    // the caret the pane then showed. And the keys that went into a
    // line that doesn't echo them.
    let mut drawn_ever: HashMap<u64, Option<Vec<char>>> = HashMap::new();
    let mut unechoed: HashSet<u64> = HashSet::new();
    let mut at_once: HashSet<u64> = HashSet::new();
    let mut firsts: HashSet<u64> = HashSet::new();
    let mut tally = Tally::default();
    let mut waking: Option<u64> = None;
    let mut on_their_way = 0usize;
    let (mut now, mut typed, mut since_enter, mut next_key) = (0u64, 0usize, 0usize, 0u64);
    // (Where others type too, what is drawn can be overtaken: only the
    // secrets are held to.)
    let mut overtaken = false;
    // (One question on its way at a time.)
    let mut asking = false;
    loop {
        events.sort_by_key(|e| (e.0, e.1));
        let mut heard = false;
        let next_event = events.first().map(|e| e.0);
        let type_now = typed < keys && next_event.is_none_or(|at| next_key <= at);
        if !type_now && next_event.is_none() {
            break;
        }
        if type_now {
            now = now.max(next_key);
            // (A line is ended before it nears the row's end: a real one
            // would wrap there, which this host can't, and nothing is
            // predicted there anyway. The rules' own tests cover it.)
            let bytes: Vec<u8> = match rng.next(100) {
                _ if since_enter >= 24 => b"\r".to_vec(),
                0..=5 => b"\r".to_vec(),
                6..=13 => vec![0x7f],
                14..=17 => b"\x1b[D".to_vec(),
                18..=20 => b"\x1b[C".to_vec(),
                21..=24 => b" ".to_vec(),
                25..=27 => b"*".to_vec(),
                28 => b"\t".to_vec(),
                _ => vec![b'a' + rng.next(26) as u8],
            };
            if since_enter == 0 && bytes.len() == 1 && bytes[0] > b' ' && bytes[0] != 0x7f {
                firsts.insert(p.writes + 1);
            }
            since_enter = if bytes == b"\r" { 0 } else { since_enter + 1 };
            let enter = bytes == b"\r";
            let (seq, _) = p.typed(&bytes, seen.as_ref(), t0 + ms(now));
            if say {
                eprintln!("{now:6} typed {seq} {:?} on {} -> {}", String::from_utf8_lossy(&bytes), text(&seen), said(&p));
            }
            drawn_ever.insert(seq, None);
            outbox.push_back((seq, bytes));
            typed += 1;
            // (After an Enter, as often as not, the pause of someone
            // reading what it brought: the host is asked about its
            // prompt in pauses.)
            let pause = if enter && rng.chance(50) {
                1200 + rng.next(2500)
            } else if rng.chance(10) {
                900
            } else {
                180
            };
            next_key = now + 20 + rng.next(pause);
            if p.overlay().is_some() && p.keys(0, p.placed()).take_while(|w| w.epoch <= p.proven).any(|w| w.seq == seq) {
                at_once.insert(seq);
            }
            if plan.others && rng.chance(3) {
                push(&mut events, now + rng.next(600), Event::Other);
            }
        } else {
            let (at, _, event) = events.remove(0);
            now = now.max(at);
            match event {
                Event::Arrives(written) => {
                    let last = written.last().unwrap().0;
                    if say {
                        eprintln!(
                            "{now:6} host takes {:?} in {:?} at {}",
                            written.iter().map(|(s, b)| format!("{s}:{}", String::from_utf8_lossy(b).escape_debug())).collect::<Vec<_>>(),
                            host.mode,
                            text(&host.screen())
                        );
                    }
                    let mut frames = Vec::new();
                    for (seq, bytes) in written {
                        // A key that lands in a line that doesn't echo it
                        // must never have been drawn.
                        let secret = host.mode != Mode::Echo && !host.asking;
                        if secret && bytes != b"\r" {
                            assert_eq!(drawn_ever[&seq], None, "session {seed}: key {seq} ({bytes:?}) was drawn, and went into a {:?} line", host.mode);
                            unechoed.insert(seq);
                        }
                        let echoes = host.mode == Mode::Echo && !host.asking && bytes.len() == 1 && bytes[0] >= b' ' && bytes[0] != 0x7f;
                        host.key(&bytes, &mut whim);
                        if echoes {
                            tally.echoed += 1;
                            tally.at_once += usize::from(at_once.contains(&seq));
                            if firsts.contains(&seq) {
                                tally.firsts += 1;
                                tally.firsts_at_once += usize::from(at_once.contains(&seq));
                            }
                        }
                        // What the pane showed with this key drawn is what
                        // the host now has, where it echoes.
                        if let (Some(Some(shown)), Some(screen), false) = (drawn_ever.get(&seq), host.screen(), overtaken || secret)
                            && host.mode == Mode::Echo
                        {
                            let has: Vec<char> = screen.before().collect();
                            assert_eq!(&has, shown, "session {seed}: key {seq} was drawn as {shown:?} and the host shows {has:?}");
                        }
                        // (A program that redraws shows each key's doing,
                        // or only where they all leave it.)
                        if whim.chance(40) {
                            frames.push(host.screen());
                        }
                    }
                    frames.push(host.screen());
                    // The answer and the frames go back: the frames a
                    // moment after the answer, or just before it; a
                    // program that draws in pieces sends one first.
                    let stall = if plan.stalls && rng.chance(4) { 400 + rng.next(2600) } else { 0 };
                    let mut back = now + delay + stall;
                    if plan.piped {
                        back = back.max(back_after);
                    }
                    push(&mut events, back + 3, Event::Answer(last));
                    let mut at = back + rng.next(9);
                    for frame in frames {
                        if plan.kind == Kind::Boxed && whim.chance(30) {
                            push(&mut events, at, Event::Frame(None));
                            at += 1 + rng.next(4);
                        }
                        push(&mut events, at, Event::Frame(frame));
                        at += 1 + rng.next(12);
                    }
                    back_after = at.max(back + 3);
                }
                Event::Answer(seq) => {
                    heard = true;
                    p.answered(seq, t0 + ms(now));
                    in_flight = false;
                    on_their_way = on_their_way.saturating_sub(1);
                }
                Event::Frame(screen) => {
                    // (A pane paints the newest of the frames that came
                    // since it last did.)
                    let more = events.iter().any(|e| matches!(e.2, Event::Frame(_)) && e.0 <= now + 8);
                    if plan.drops && more && rng.chance(50) {
                        continue;
                    }
                    seen = screen;
                    heard = true;
                    p.frame(seen.as_ref(), t0 + ms(now));
                    if say {
                        eprintln!("{now:6} frame {} -> {}", text(&seen), said(&p));
                    }
                }
                Event::Other => {
                    overtaken = true;
                    if host.kind == Kind::Shell && whim.chance(30) && host.mode == Mode::Echo {
                        host.prompt = "(main) $ ".chars().collect();
                    } else {
                        host.key(&[b'A' + whim.next(26) as u8], &mut whim);
                    }
                    push(&mut events, now + delay + rng.next(9), Event::Frame(host.screen()));
                }
                Event::Tick => {
                    waking = None;
                    // (The app asks then too, where the clock gave up
                    // the last key that waited.)
                    heard = true;
                }
                Event::Asked(token) => {
                    let reads = host.shell_reads();
                    // (Its answer on a connection of its own: it may
                    // pass the frames, or come after them.)
                    push(&mut events, now + delay + rng.next(40), Event::Said(token, reads));
                }
                Event::Said(token, reads) => {
                    asking = false;
                    let trusted = p.prompt_said(token, reads, seen.as_ref());
                    if say {
                        eprintln!("{now:6} host says the shell {} -> {} ({})", if reads { "reads" } else { "doesn't read" }, said(&p), if trusted { "trusted" } else { "not trusted" });
                    }
                }
            }
        }
        // At a pause in what the host shows, where the caret stands at
        // a prompt known to echo and nothing typed is on its way, the
        // host is asked whether its shell is reading.
        // (Once for each time something came from the host: the app
        // asks at a pause in a terminal's output, not on a clock. Asked
        // again at every turn of this loop, a session whose host said
        // no never ended.)
        if heard
            && !asking
            && let Some(token) = p.wants_prompt(seen.as_ref())
        {
            asking = true;
            push(&mut events, now + 150 + delay, Event::Asked(token));
        }
        // One write at a time, all that waits gathered into it; or,
        // with the helper on the host, each as it is typed (no more
        // than the app has on their way at once: the rest wait, and go
        // together).
        if plan.piped {
            while !outbox.is_empty() && on_their_way < 32 {
                let written: Vec<_> = if on_their_way == 31 { outbox.drain(..).collect() } else { outbox.drain(..1).collect() };
                on_their_way += 1;
                p.went(written.last().unwrap().0, t0 + ms(now));
                let stall = if plan.stalls && rng.chance(4) { 400 + rng.next(2600) } else { 0 };
                arrives_after = arrives_after.max(now + delay + stall);
                push(&mut events, arrives_after, Event::Arrives(written));
            }
        } else if !in_flight && !outbox.is_empty() {
            in_flight = true;
            let written: Vec<_> = outbox.drain(..).collect();
            p.went(written.last().unwrap().0, t0 + ms(now));
            let stall = if plan.stalls && rng.chance(4) { 400 + rng.next(2600) } else { 0 };
            push(&mut events, now + delay + stall, Event::Arrives(written));
        }
        // What time brings, when it brings it.
        if let (Some(next), _) = p.tick(t0 + ms(now)) {
            let at = (next - t0).as_millis() as u64 + 1;
            if waking.is_none_or(|w| at < w) {
                waking = Some(at);
                push(&mut events, at, Event::Tick);
            }
        }
        // What is drawn now: no key that went where nothing echoes, and
        // each key drawn noted with the text the pane shows before its
        // caret once that key is in.
        if let Some(o) = p.overlay() {
            let n = p.keys(0, p.placed()).take_while(|w| w.epoch <= p.proven).count();
            let line = p.line(n).unwrap();
            assert_eq!(o.caret, line.col);
            assert!((o.col as usize) + o.cells.len() <= WIDTH && (o.caret as usize) < WIDTH, "session {seed}: drawn past the row: {o:?}");
            let seqs: Vec<u64> = p.keys(0, n).map(|w| w.seq).collect();
            for (i, seq) in seqs.into_iter().enumerate() {
                assert!(!unechoed.contains(&seq), "session {seed}: key {seq} is drawn, and went into a line that doesn't echo");
                let line = p.line(i + 1).unwrap();
                drawn_ever.insert(seq, Some(line.cells[..line.col as usize].iter().map(|c| c.0.ch).collect()));
            }
        }
    }
    // Long after: nothing drawn, nothing waiting.
    p.frame(host.screen().as_ref(), t0 + ms(now + 5_000));
    p.tick(t0 + ms(now + 10_000));
    assert!(p.overlay().is_none() && !p.waits(), "session {seed}: left at the end: {} (host {})", said(&p), text(&host.screen()));
    tally
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn said(p: &Predictor) -> String {
    let sent: Vec<String> = p
        .sent
        .iter()
        .map(|s| match s {
            Sent::Key(w) => format!("{:?}#{}e{}", w.key, w.seq, w.epoch),
            Sent::Gap(g) => format!("gap#{g}"),
        })
        .collect();
    format!("sent {sent:?} epoch {} proven {} floor {} right {} doubt {} taint {}", p.epoch, p.proven, p.floor, p.right, p.doubt.is_some(), p.taint.is_some())
}

/// (The sessions from 2,400 on are the first 2,400 again, each key
/// sent as it is typed.)
fn plan(n: u64) -> Plan {
    let (n, piped) = (n % 2400, n >= 2400);
    Plan {
        seed: n,
        kind: if n % 3 == 2 { Kind::Boxed } else { Kind::Shell },
        delay: [4, 35, 90, 200, 450][(n / 3 % 5) as usize],
        stalls: n / 15 % 2 == 1,
        drops: n / 30 % 2 == 1,
        others: n / 60 % 4 == 3,
        piped,
    }
}

#[test]
fn sessions_played_by_the_thousand() {
    if let Some(only) = std::env::var("PLAY_ONLY").ok().and_then(|v| v.parse().ok()) {
        eprintln!("{:?}: {:?}", plan(only), play(plan(only), 160));
        return;
    }
    let mut tallies: HashMap<(bool, bool, u64, bool), Tally> = HashMap::new();
    for n in 0..4800 {
        let plan = plan(n);
        let tally = play(plan, 160);
        let sum = tallies.entry((plan.piped, plan.kind == Kind::Boxed, plan.delay, plan.stalls || plan.others)).or_default();
        sum.echoed += tally.echoed;
        sum.at_once += tally.at_once;
        sum.firsts += tally.firsts;
        sum.firsts_at_once += tally.firsts_at_once;
    }
    // Most of what is typed where it echoes is drawn at once, on every
    // link slow enough for it to be drawn at all; and nothing is on a
    // quick one that doesn't stall.
    let mut all: Vec<_> = tallies.into_iter().collect();
    all.sort_by_key(|(key, _)| *key);
    for ((piped, boxed, delay, rough), tally) in all {
        let share = tally.at_once as f64 / tally.echoed.max(1) as f64;
        eprintln!(
            "{}{} at {delay} ms each way{}: {} of {} letters drawn at once ({:.0}%); of a line's first letters {} of {} ({:.0}%)",
            if piped { "each key sent as typed: " } else { "" },
            if boxed { "a drawn caret" } else { "a shell" },
            if rough { ", stalling or shared" } else { "" },
            tally.at_once,
            tally.echoed,
            share * 100.0,
            tally.firsts_at_once,
            tally.firsts,
            tally.firsts_at_once as f64 / tally.firsts.max(1) as f64 * 100.0
        );
        if delay >= 35 && !rough {
            // (Less on a slower link: after each key that isn't
            // predicted, what is typed is unseen until a letter of it is
            // back, and on a slow link much of a short line is typed by
            // then. The host's half is to change that: TODO.)
            let least = match (boxed, delay) {
                (false, 35) => 0.78,
                (false, 90) => 0.64,
                (false, 200) => 0.44,
                (false, _) => 0.20,
                (true, 35) => 0.65,
                (true, 90) => 0.45,
                (true, 200) => 0.30,
                (true, _) => 0.13,
            };
            assert!(share > least, "too careful to be of use at {delay} ms: {share:.2}");
            // And a shell's prompt is known again: of the first letters
            // typed at a line (after an Enter, as often as not after
            // the pause of someone reading; three lines in ten read a
            // secret), a share is drawn at once, the host having said
            // its shell reads there. (Less on a slower link: the
            // question takes a round trip, which the host's half is to
            // save: TODO.)
            let firsts = tally.firsts_at_once as f64 / tally.firsts.max(1) as f64;
            let least = match (boxed, delay) {
                (false, 35) => 0.22,
                (false, 90) => 0.14,
                (false, 200) => 0.07,
                _ => 0.0,
            };
            assert!(firsts >= least, "a line's first letter is drawn at once too seldom at {delay} ms: {firsts:.2}");
            if boxed {
                assert_eq!(tally.firsts_at_once, 0, "a program that draws its own caret isn't a shell at its prompt");
            }
        } else if !rough {
            assert_eq!(tally.at_once, 0, "an echo this quick is the host's to draw");
        }
    }
}

/// Anything typed on anything: screens of every size and state, keys
/// of every kind, frames and answers in any order. Nothing panics, and
/// nothing is drawn past its row.
#[test]
fn anything_typed_on_anything() {
    let mut rng = Lcg(0xfeed);
    let t0 = Instant::now();
    let letters: Vec<char> = "ab *│$>日é\u{FFFE}\u{FFFF} ".chars().collect();
    let screen = |rng: &mut Lcg| {
        if rng.chance(8) {
            return None;
        }
        let width = 1 + rng.next(14) as usize;
        let cells = (0..width).map(|_| Cell { ch: letters[rng.next(letters.len() as u64) as usize], dim: rng.chance(15), ground: rng.chance(10) }).collect();
        Some(Screen { row: rng.next(5) as u16, col: rng.next(width as u64 + 2) as u16, cells, drawn: rng.chance(30), shape: rng.next(3) as u8, alt: rng.chance(10) })
    };
    for _ in 0..400 {
        let mut p = Predictor::new(true);
        let mut now = 0u64;
        // (A screen is kept for a while: typing happens on one.)
        let mut on = screen(&mut rng);
        for _ in 0..300 {
            now += rng.next(120);
            if rng.chance(25) {
                on = screen(&mut rng);
            }
            match rng.next(12) {
                0..=4 => {
                    let bytes: &[u8] =
                        [&b"a"[..], b"b", b" ", b"*", b"\x7f", b"\x1b[D", b"\x1b[C", b"\x1b[3~", b"\r", b"\t", "日".as_bytes(), "é".as_bytes(), b"ab", b"\xff", b""][rng.next(15) as usize];
                    p.typed(bytes, on.as_ref(), t0 + ms(now));
                }
                5..=8 => {
                    p.frame(on.as_ref(), t0 + ms(now));
                }
                9 => {
                    let seq = p.writes.saturating_sub(rng.next(3));
                    if rng.chance(50) {
                        p.went(seq, t0 + ms(now));
                    } else {
                        p.answered(seq, t0 + ms(now));
                    }
                }
                10 => {
                    p.set_secret(rng.chance(30));
                }
                _ => {
                    p.tick(t0 + ms(now));
                }
            }
            if let (Some(o), Some(base)) = (p.overlay(), &p.base) {
                assert!((o.col as usize) + o.cells.len() <= base.cells.len() && (o.caret as usize) <= base.cells.len(), "{o:?} on {} cells", base.cells.len());
            }
        }
    }
}
