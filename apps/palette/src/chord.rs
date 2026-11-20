//! What a group of shortcuts can be finished with, shown while it waits
//! for its second chord (app_server's `chord`: ⌃⌥⌘Z begins Zoom's, and
//! Z, V, H or B picks from it). app_server starts `olo-palette --chord
//! FIRST [OUTPUT]` once a group has waited a moment (keys typed straight
//! through show nothing), which hands it to the running copy: a small
//! panel at the foot of the display, the group's name, each second key
//! on a cap and what it does, as the shortcuts sheet names them.
//!
//! It takes no keys and no pointer: app_server has the keyboard's next
//! key (it picks, or lets go), and the panel goes when app_server says
//! the group waits no more (`Event::Chord`), or it has waited as long
//! as a group can. Agents read it as any window: a heading and a label
//! a key ("V: Zoom vertically").

use std::time::Duration;

use olo_kit::{
    Cx, Element, FontWeight, LayerKeyboard, ParentElement as _, Proxy, Styled as _, View, WindowHandle, WindowOptions, canvas, div, metrics as m, px,
    style::{self, c},
    theme,
};
use olo_protocol::{ChordInfo, Client, Event, EventKind, Reply, Request};

/// What `--chord` asked for: the first chord, as profiles write it, and
/// the display to show on.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub first: String,
    pub output: Option<String>,
}

impl Ask {
    pub fn parse(args: &[String]) -> Option<Ask> {
        let i = args.iter().position(|a| a == "--chord")?;
        Some(Ask { first: args.get(i + 1)?.clone(), output: args.get(i + 2).filter(|o| !o.starts_with("--")).cloned() })
    }
}

/// A group that's waiting, to show; and which asking it was.
pub struct Learned {
    pub info: ChordInfo,
    pub output: Option<String>,
    pub turn: u64,
}

/// The group asked about at `turn` waits no more.
pub struct Ended(pub u64);

/// How long a panel stays at the most: as long as a group waits
/// (app_server's `keymap::CHORD_WAIT`) and a little, in case its word
/// that it ended never came.
const STAYS: Duration = Duration::from_millis(5500);

/// Learn what `ask`'s chord begins and send [`Learned`] if it's still
/// waiting, then [`Ended`] when app_server says it is no longer.
pub fn learn(ask: Ask, turn: u64, proxy: Proxy) {
    let _ = std::thread::Builder::new().name("palette-chord".into()).spawn(move || {
        // Listening before asking: an end that came between the two is
        // heard.
        let Some(events) = Client::connect().ok().and_then(|c| c.into_events_of(&[EventKind::Chord]).ok()) else { return };
        let info = match Client::connect().ok().and_then(|mut c| c.request(&Request::Chord { first: Some(ask.first.clone()) }).ok()) {
            Some(Reply::Chord { chord }) if chord.waiting && !chord.options.is_empty() => chord,
            _ => return,
        };
        proxy.send(Learned { info, output: ask.output, turn });
        let late = proxy.clone();
        let _ = std::thread::Builder::new().name("palette-chord-late".into()).spawn(move || {
            std::thread::sleep(STAYS);
            late.send(Ended(turn));
        });
        for e in events.flatten() {
            if matches!(e, Event::Chord { .. }) {
                proxy.send(Ended(turn));
                break;
            }
        }
    });
}

/// The panel.
pub struct Panel {
    info: ChordInfo,
    /// As wide as what it says (`width`).
    width: f32,
}

/// The least and the most a panel is wide: a short group isn't a strip,
/// and a long title is cut short before the panel is a banner.
const WIDTH_LEAST: f32 = 220.0;
const WIDTH_MOST: f32 = 420.0;
const PAD: f32 = 14.0;
/// Above the heading and under the foot: less than at the sides, whose
/// words start at the pad itself (the heading's and the foot's sit
/// inside their rows), so the room round the words is the same all
/// round.
const PAD_V: f32 = 10.0;
const HEAD: f32 = 24.0;
const ROW: f32 = m::CONTROL_HEIGHT;
/// A key's cap: as the kit's key hints' (a check box's row), and the
/// room between it and what its key does.
const CAP: f32 = m::MARK_ROW;
const CAP_GAP: f32 = 8.0;
const FOOT: f32 = 24.0;
/// How far the heading's rule is from the rows, and the foot's: the
/// same above the first cap as under the last.
const RULE: f32 = 5.0;
/// The room round the panel for its shadow, and how far it stands off
/// the display's foot.
const SHADOW: f32 = 8.0;
const LIFT: i32 = 56;
const FOOT_SAYS: &str = "Esc cancels";
/// The room a checked row keeps at its right for its mark.
const CHECK: f32 = 18.0;

/// What app_server said of a group or a shortcut (their names are the
/// protocol's, in English, and come over the socket as text) in the
/// person's language.
fn said(text: &str) -> olo_kit::SharedString {
    olo_kit::words::lookup(text).map_or_else(|| text.to_owned().into(), Into::into)
}

fn height(options: usize) -> f32 {
    2.0 * PAD_V + HEAD + options as f32 * ROW + FOOT
}

/// The caps' column: as wide as the widest key's cap, on the two-point
/// grid.
fn cap_width(info: &ChordInfo) -> f32 {
    let bold = style::ui_font(FontWeight::BOLD);
    let w = info.options.iter().map(|o| f32::from(style::text_width(o.label.clone(), style::SMALL_SIZE, bold.clone())) + 2.0 * m::TEXT_INSET).fold(CAP, f32::max);
    (w / 2.0).ceil() * 2.0
}

/// As wide as what it says: its longest row, or its heading with the
/// keys that began the group (a group of four short names was a third
/// words and two thirds nothing at one width for all; Opus's look,
/// 2026-11-20).
fn width(info: &ChordInfo) -> f32 {
    let (plain, bold) = (style::ui_font(FontWeight::NORMAL), style::ui_font(FontWeight::BOLD));
    let w = |t: olo_kit::SharedString, size: f32, font: &olo_kit::Font| f32::from(style::text_width(t, size, font.clone()));
    let rows = info.options.iter().map(|o| w(said(&o.title), style::PLAIN_SIZE, &plain) + if o.current { CHECK } else { 0.0 }).fold(0.0, f32::max) + cap_width(info) + CAP_GAP;
    let head = info.name.as_ref().map_or(0.0, |n| w(said(n), style::PLAIN_SIZE, &bold) + 2.0 * m::SPACING) + style::key_combo_width(&info.label);
    let foot = w(olo_kit::words::tr(FOOT_SAYS).into(), style::DETAIL_SIZE, &plain);
    let wide = rows.max(head).max(foot) + 2.0 * PAD;
    (wide.clamp(WIDTH_LEAST, WIDTH_MOST) / 2.0).ceil() * 2.0
}

impl View for Panel {
    type Message = ();

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        let name: String = self.info.name.clone().unwrap_or_default();
        let first = self.info.label.clone();
        let rows = self.info.options.len();
        // (As a menu's separator: a line cut into the ground.)
        fn etched(g: &mut olo_kit::Painter, b: olo_kit::Bounds<olo_kit::Pixels>, y: olo_kit::Pixels) {
            let p = theme::palette();
            style::rule(g, b.left(), b.right(), y, style::Rule::Etched, style::shade(&p, p.panel_background, 1.16), &p);
        }
        let heading = canvas(move |g, b| {
            let p = theme::palette();
            // (As far over its rule as the first cap is under it.)
            let y = b.top() + px(HEAD / 2.0 - 4.0);
            let keys_w = style::key_combo_width(&first);
            if !name.is_empty() {
                let said = said(&name);
                let r =
                    style::label_fit(g, b.left(), y, style::Align::Left, said.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), c(p.panel_text), b.size.width - px(keys_w + m::SPACING));
                g.agent(olo_kit::agent::Node { role: "Heading", name: Some(said), bounds: r, enabled: true, ..Default::default() });
            }
            // The chord that began it, at the right as a menu's keys are.
            let r = style::key_combo(g, olo_kit::point(b.right(), y), &first, style::secondary_text(&p));
            g.agent(olo_kit::agent::Node::label(first.clone(), r));
            etched(g, b, b.bottom() - px(RULE));
        })
        .paint_key(style::key((&self.info.name, &self.info.label)))
        .w_full()
        .h(px(HEAD))
        .flex_shrink_0();
        // Each second key on a cap (as the kit's key hints draw theirs),
        // the caps one column, as wide as the widest, so what they do
        // reads down one edge.
        let options: Vec<(olo_kit::SharedString, olo_kit::SharedString, bool)> = self.info.options.iter().map(|o| (o.label.clone().into(), said(&o.title), o.current)).collect();
        let bold = style::ui_font(FontWeight::BOLD);
        let cap_w = cap_width(&self.info);
        let list = canvas(move |g, b| {
            let p = theme::palette();
            let plain = style::ui_font(FontWeight::NORMAL);
            for (i, (key, title, current)) in options.iter().enumerate() {
                // (What's so already steps back, as its menu's item is
                // checked and dimmed: the workspace the window is on.)
                let (ink, cap_ink) = if *current { (style::disabled_text(&p), style::disabled_text(&p)) } else { (c(p.panel_text), c(p.control_text)) };
                let cy = b.top() + px(i as f32 * ROW + ROW / 2.0);
                let baseline = style::baseline_at(g, cy, style::PLAIN_SIZE);
                let cap = style::snap(g, olo_kit::Bounds::new(olo_kit::point(b.left(), cy - px(CAP / 2.0)), olo_kit::size(px(cap_w), px(CAP))));
                style::fill_rect(g, cap, c(p.control_background));
                style::raised(g, cap, &p);
                // (In the middle of its cap, not on the words' baseline:
                // smaller than they are, it sat low in the cap.)
                style::label(g, cap.center().x, cap.center().y, style::Align::Center, key.clone(), style::SMALL_SIZE, bold.clone(), cap_ink);
                let r = style::label_on_baseline_fit(
                    g,
                    b.left() + px(cap_w + CAP_GAP),
                    baseline,
                    style::Align::Left,
                    title.clone(),
                    style::PLAIN_SIZE,
                    plain.clone(),
                    ink,
                    b.size.width - px(cap_w + CAP_GAP + if *current { CHECK } else { 0.0 }),
                );
                // (And is checked, as the menu's is: stepped back alone
                // read as something that can't be done.)
                if *current {
                    let mark = style::snap(g, olo_kit::Bounds::new(olo_kit::point(b.right() - px(10.0), cy - px(4.5)), olo_kit::size(px(9.0), px(9.0))));
                    style::check_mark(g, mark, ink);
                }
                let row = olo_kit::Bounds::from_corners(olo_kit::point(cap.left(), cap.top().min(r.top())), olo_kit::point(r.right(), cap.bottom().max(r.bottom())));
                g.agent(olo_kit::agent::Node { enabled: !*current, ..olo_kit::agent::Node::label(format!("{key}: {title}"), row) });
            }
        })
        .paint_key(style::key((&self.info.first, rows, cap_w.to_bits(), self.info.options.iter().position(|o| o.current))))
        .w_full()
        .h(px(rows as f32 * ROW))
        .flex_shrink_0();
        // The way out, under a rule as far from the last cap as the
        // heading's is from the first.
        let foot = canvas(move |g, b| {
            let p = theme::palette();
            etched(g, b, b.top() + px(RULE));
            let said = olo_kit::words::tr(FOOT_SAYS);
            let r = style::label(g, b.left(), b.top() + px(RULE + 12.0), style::Align::Left, said, style::DETAIL_SIZE, style::ui_font(FontWeight::NORMAL), style::secondary_text(&p));
            g.agent(olo_kit::agent::Node::label(said, r));
        })
        .paint_key(0)
        .w_full()
        .h(px(FOOT))
        .flex_shrink_0();
        let panel = canvas(move |g, b| {
            let p = theme::palette();
            let panel = style::snap(g, style::inset(g, b, SHADOW));
            style::soft_shadow(g, panel, &p);
            style::fill_rect(g, panel, c(p.panel_background));
            style::frame_rect(g, panel, style::tint(p.panel_background, 1.6));
            style::raised(g, style::inset(g, panel, 1.0), &p);
        })
        .paint_key(style::key((rows, self.width.to_bits())))
        .absolute()
        .inset_0();
        div().size_full().relative().child(panel).child(div().absolute().inset_0().px(px(SHADOW + PAD)).py(px(SHADOW + PAD_V)).flex().flex_col().child(heading).child(list).child(foot))
    }
}

/// The panel for `learned`, at the foot of its display.
pub fn open(learned: Learned) -> WindowHandle<Panel> {
    let output = learned.output.clone().filter(|o| olo_kit::display_names().iter().any(|d| d.as_deref() == Some(o.as_str())));
    let width = width(&learned.info);
    let (w, h) = (width + 2.0 * SHADOW, height(learned.info.options.len()) + 2.0 * SHADOW);
    let options = WindowOptions::new("Shortcuts".to_owned(), w, h).no_panel().layer("chord", olo_kit::LayerShellLayer::Overlay, (false, true, false, false)).layer_options(move |l| {
        l.output = output;
        l.margin = (0, 0, LIFT, 0);
        l.keyboard = LayerKeyboard::None;
    });
    olo_kit::open_window(options, move |_| Panel { info: learned.info, width })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks() {
        let args = |s: &str| s.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(Ask::parse(&args("--chord Ctrl+Alt+Cmd+Z DP-2")), Some(Ask { first: "Ctrl+Alt+Cmd+Z".into(), output: Some("DP-2".into()) }));
        assert_eq!(Ask::parse(&args("--chord Ctrl+Alt+Cmd+Z")), Some(Ask { first: "Ctrl+Alt+Cmd+Z".into(), output: None }));
        assert_eq!(Ask::parse(&args("--toggle")), None);
    }
}
