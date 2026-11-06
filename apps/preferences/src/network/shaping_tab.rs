//! Network ▸ Shaping, as a Mac's Network Link Conditioner: a link made
//! slower, later and lossier than it is, what it receives and what it
//! sends each their own (bandwidth, delay, jitter, loss), from a network
//! to try a program on (Edge, 3G, LTE…) or one's own numbers; turned on
//! and off as root (`olo_system::shaping`). It lasts until turned off,
//! the link goes away or the computer restarts.

use olo_system::{
    network::{Kind, Status},
    shaping::{PRESETS, Shape, Shaping},
};
use olo_ui::{
    gpui::{App, Context, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, div, px},
    metrics as m,
    widgets::{Button, MenuField, StringView, TextControl, TextField},
};

use super::{Extra, Request, State, field, state_of};
use crate::{PanelView, form::Ui};

const DIVIDER: f32 = 80.0;

/// One way's fields: bandwidth (kbit/s), delay and jitter (ms), loss (%).
struct Way([Entity<TextField>; 4]);

impl Way {
    fn new(cx: &mut Context<PanelView>) -> Way {
        Way([field(cx, "As it goes"), field(cx, "0"), field(cx, "0"), field(cx, "0")])
    }

    fn shape(&self, cx: &App) -> Result<Option<Shape>, String> {
        let text = |i: usize| self.0[i].read(cx).text().trim().to_owned();
        let whole = |i: usize, what: &str| -> Result<u64, String> {
            let t = text(i);
            if t.is_empty() { Ok(0) } else { t.parse::<u64>().map_err(|_| format!("The {what} is a whole number.")) }
        };
        let loss = match text(3) {
            t if t.is_empty() => 0.0,
            t => t.trim_end_matches('%').trim().parse::<f32>().map_err(|_| "The loss is a number, in percent.".to_owned())?,
        };
        let s = Shape { rate_kbit: whole(0, "bandwidth")?, delay_ms: whole(1, "delay")?.min(u32::MAX as u64) as u32, jitter_ms: whole(2, "jitter")?.min(u32::MAX as u64) as u32, loss };
        s.check().map_err(|e| format!("{}{}.", e[..1].to_uppercase(), &e[1..]))?;
        Ok((!s.is_nothing()).then_some(s))
    }

    fn fill(&self, s: Option<Shape>, cx: &mut App) {
        let s = s.unwrap_or_default();
        let nonzero = |v: u64| if v == 0 { String::new() } else { v.to_string() };
        let texts = [nonzero(s.rate_kbit), nonzero(s.delay_ms as u64), nonzero(s.jitter_ms as u64), if s.loss > 0.0 { format!("{}", s.loss) } else { String::new() }];
        for (f, t) in self.0.iter().zip(texts) {
            f.update(cx, |f, cx| f.set_text(t, cx));
        }
    }

    fn ui(&self, ui: &mut Ui) {
        for (f, label) in self.0.iter().zip(["Bandwidth:", "Delay:", "Jitter:", "Loss:"]) {
            let unit = match label {
                "Bandwidth:" => "kbit/s",
                "Loss:" => "%",
                _ => "ms",
            };
            ui.add(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::LABEL_GAP))
                    .child(div().flex_1().child(TextControl::new(f.clone()).label(label).divider(DIVIDER)))
                    .child(div().w(px(40.0)).child(StringView::new(unit))),
            );
        }
    }
}

pub struct Edit {
    device: usize,
    down: Way,
    up: Way,
    /// What the fields were filled from (the link, its shaping).
    loaded: Option<(String, Shaping)>,
}

impl Edit {
    pub fn new(cx: &mut Context<PanelView>) -> Edit {
        Edit { device: 0, down: Way::new(cx), up: Way::new(cx), loaded: None }
    }

    fn edited(&self, cx: &App) -> Result<Shaping, String> {
        Ok(Shaping { down: self.down.shape(cx)?, up: self.up.shape(cx)? })
    }
}

fn way_text(s: &Shape) -> String {
    let mut parts = Vec::new();
    if s.rate_kbit > 0 {
        parts.push(if s.rate_kbit >= 1000 && s.rate_kbit.is_multiple_of(1000) { format!("{} Mbit/s", s.rate_kbit / 1000) } else { format!("{} kbit/s", s.rate_kbit) });
    }
    if s.delay_ms > 0 {
        parts.push(if s.jitter_ms > 0 { format!("{} ± {} ms", s.delay_ms, s.jitter_ms) } else { format!("{} ms", s.delay_ms) });
    }
    if s.loss > 0.0 {
        parts.push(format!("{}% lost", s.loss));
    }
    parts.join(", ")
}

pub fn ui(ui: &mut Ui, st: &mut State, status: &Status, extra: &Extra, busy: bool) {
    let links: Vec<_> = status.devices.iter().filter(|d| d.kind != Kind::Other).cloned().collect();
    if links.is_empty() {
        return ui.note("There are no links to shape.");
    }
    let e = &mut st.shape;
    e.device = e.device.min(links.len() - 1);
    let names: Vec<SharedString> = links.iter().map(|d| SharedString::from(format!("{} ({})", d.interface, if d.kind == Kind::Wifi { "Wi-Fi" } else { "Ethernet" }))).collect();
    let pick = ui.on_view(|v, i: usize, _, cx| {
        if let Some(st) = state_of(v) {
            st.shape.device = i;
        }
        cx.notify();
    });
    ui.add(MenuField::new("shaping-link", names, e.device).label("Link:").divider(DIVIDER).on_change(pick));
    let iface = links[e.device].interface.clone();
    let now = extra.shaping.iter().find(|(i, _)| *i == iface).map(|(_, s)| *s).unwrap_or_default();
    if e.loaded.as_ref() != Some(&(iface.clone(), now)) {
        e.down.fill(now.down, ui.cx);
        e.up.fill(now.up, ui.cx);
        e.loaded = Some((iface.clone(), now));
    }
    let state = match (now.down, now.up) {
        (None, None) => "Not shaped: as fast as it goes.".to_owned(),
        (d, u) => {
            let way = |s: Option<Shape>| s.map(|s| way_text(&s)).unwrap_or_else(|| "as it goes".into());
            format!("Shaped. Received: {}. Sent: {}.", way(d), way(u))
        }
    };
    ui.add(super::labelled(DIVIDER, "Now:", StringView::new(state)));
    // A network to start from, named while the fields are its.
    let edited = st.shape.edited(ui.cx);
    let like = edited.as_ref().ok().and_then(|s| PRESETS.iter().position(|p| s.down == Some(p.down) && s.up == Some(p.up))).map_or(0, |i| i + 1);
    let first = if like == 0 && edited.as_ref().is_ok_and(|s| s.is_on()) { "Custom" } else { "Choose…" };
    let mut presets: Vec<SharedString> = vec![first.into()];
    presets.extend(PRESETS.iter().map(|p| SharedString::from(p.name)));
    let fill = ui.on_view(|v, i: usize, _, cx| {
        let Some(st) = state_of(v) else { return };
        if let Some(p) = i.checked_sub(1).and_then(|i| PRESETS.get(i)) {
            st.shape.down.fill(Some(p.down), cx);
            st.shape.up.fill(Some(p.up), cx);
        }
        cx.notify();
    });
    ui.add(MenuField::new("shaping-preset", presets, like).label("Like:").divider(DIVIDER).on_change(fill));
    let (down, up) = (ui.collect(|ui| ui.bbox("Received", |ui| st.shape.down.ui(ui))), ui.collect(|ui| ui.bbox("Sent", |ui| st.shape.up.ui(ui))));
    ui.add_columns(down, up);
    if let Err(why) = &edited {
        ui.label(why.clone());
    }
    let changed = edited.as_ref().is_ok_and(|s| *s != now);
    let off_iface = iface.clone();
    let off = ui.on_click_view(move |v, _, cx| {
        if let Some(st) = state_of(v) {
            let _ = st.link.tx.send(Request::Shape { interface: off_iface.clone(), shaping: Shaping::default() });
        }
        cx.notify();
    });
    let apply = ui.on_click_view(move |v, _, cx| {
        let Some(st) = state_of(v) else { return };
        if let Ok(shaping) = st.shape.edited(cx) {
            let _ = st.link.tx.send(Request::Shape { interface: iface.clone(), shaping });
        }
        cx.notify();
    });
    let label = if now.is_on() { "Apply" } else { "Turn On" };
    ui.add(
        div()
            .flex()
            .flex_row()
            .gap(px(m::SPACING))
            .child(div().flex_1())
            .child(Button::new("Turn Off").enabled(now.is_on() && !busy).on_click(off))
            .child(Button::new(label).default(changed).enabled(changed && edited.as_ref().is_ok_and(|s| s.is_on()) && !busy).on_click(apply))
            .into_any_element(),
    );
    ui.note("It lasts until it's turned off, the link goes away or the computer restarts. Changing it asks for an administrator's password once while this window is open.");
}
