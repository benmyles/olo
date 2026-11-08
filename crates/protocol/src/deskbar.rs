//! Deskbar's live widgets (Preferences ▸ Deskbar; Ben, 2026-11-07: "add
//! a preferences app for the deskbar and in it support configuring/adding
//! live performance widgets into it e.g. cpu, gpu, disk, network monitors
//! etc. -- it should all be very customizable in terms of where they
//! appear on the deskbar, how they look, how often they update, exactly
//! what to show and so on"). As BeOS's Pulse and ProcessController put
//! their bars in the Deskbar's tray: each widget measures one thing
//! (`Measure`), is drawn one way (`Look`), somewhere (`Place`), read as
//! often as it says. Kept in app_server's settings, so Deskbar follows a
//! change at once and agents read and set them (`SetSettings`).

use olo_theme::Hue;
use serde::{Deserialize, Serialize};

/// The fastest and slowest a widget is read.
pub const MIN_EVERY_MS: u32 = 250;
pub const MAX_EVERY_MS: u32 = 60_000;
/// The longest a graph looks back, in seconds.
pub const MAX_SPAN_S: u32 = 3600;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DeskbarSettings {
    /// Its widgets, in order: each place shows its own in this order.
    pub widgets: Vec<Widget>,
}

impl DeskbarSettings {
    /// An id no widget has.
    pub fn next_id(&self) -> u32 {
        self.widgets.iter().map(|w| w.id).max().map_or(1, |m| m + 1)
    }

    pub fn get(&self, id: u32) -> Option<&Widget> {
        self.widgets.iter().find(|w| w.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Widget> {
        self.widgets.iter_mut().find(|w| w.id == id)
    }
}

/// One live widget.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Widget {
    /// The same as it's edited and moved (Preferences' selection, agents).
    pub id: u32,
    pub measure: Measure,
    pub place: Place,
    pub look: Look,
    /// A row's height (a tray widget is an icon's).
    pub size: Size,
    /// In a row, its label ("CPU") and its value ("23%"); a tray widget
    /// says both in its tip.
    pub label: bool,
    pub value: bool,
    /// How often it's read, in milliseconds (`MIN_EVERY_MS`..`MAX_EVERY_MS`).
    pub every_ms: u32,
    /// How far back a graph looks, in seconds.
    pub span_s: u32,
    /// Its colour: one of the look's hues, or None for the selection's
    /// accent.
    pub hue: Option<Hue>,
    /// In the row of the widget before it (in the same place), side by
    /// side, each drawn small: its label over its drawing, as iStat
    /// Menus' are. Ignored in the tray and for a place's first.
    pub beside: bool,
}

impl Default for Widget {
    fn default() -> Self {
        Widget::new(0, Measure::Cpu { cores: false })
    }
}

impl Widget {
    /// A new widget measuring `measure`, as it comes: in a row under the
    /// tray, a graph of the last minute read every second, with its label
    /// and value, in the hue `measure` has.
    pub fn new(id: u32, measure: Measure) -> Widget {
        let look = match measure {
            Measure::Cpu { cores: true } => Look::Bars,
            Measure::Space { .. } | Measure::Memory { .. } => Look::Meter,
            _ => Look::Graph,
        };
        Widget { id, hue: Some(measure.hue()), measure, place: Place::Top, look, size: Size::Medium, label: true, value: true, every_ms: 1000, span_s: 60, beside: false }
    }

    /// `every_ms` within what's allowed.
    pub fn every(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.every_ms.clamp(MIN_EVERY_MS, MAX_EVERY_MS) as u64)
    }

    /// How many readings its graph keeps (at least two).
    pub fn history(&self) -> usize {
        ((self.span_s.clamp(1, MAX_SPAN_S) as u64 * 1000) / self.every().as_millis().max(1) as u64).clamp(2, 4096) as usize
    }
}

/// What a widget measures.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Measure {
    /// The processors: all together, or each core.
    Cpu { cores: bool },
    /// Memory in use (and swap's, a second series).
    Memory { swap: bool },
    /// A GPU's work (its PCI address, "0000:00:02.0"; None: the one the
    /// computer started on).
    Gpu { pci: Option<String> },
    /// A disk's reads and writes, per second (its block device,
    /// "nvme0n1"; None: every disk).
    Disk { device: Option<String>, read: bool, write: bool },
    /// How full a volume is (its mount point).
    Space { mount: String },
    /// A network interface's traffic, per second ("wlp0s20f3"; None:
    /// every interface but loopback).
    Network { interface: Option<String>, down: bool, up: bool },
    /// A temperature ("coretemp/Package id 0": a hwmon chip's name and
    /// its sensor's label; None: the processor's).
    Temperature { sensor: Option<String> },
    /// What the computer draws from its battery, in watts.
    Power,
}

impl Measure {
    /// Its kind's name ("CPU"), for menus and labels.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Measure::Cpu { .. } => "CPU",
            Measure::Memory { .. } => "Memory",
            Measure::Gpu { .. } => "GPU",
            Measure::Disk { .. } => "Disk",
            Measure::Space { .. } => "Disk Space",
            Measure::Network { .. } => "Network",
            Measure::Temperature { .. } => "Temperature",
            Measure::Power => "Power",
        }
    }

    /// The hue a new widget of it gets.
    pub fn hue(&self) -> Hue {
        match self {
            Measure::Cpu { .. } => Hue::Blue,
            Measure::Memory { .. } => Hue::Green,
            Measure::Gpu { .. } => Hue::Purple,
            Measure::Disk { .. } => Hue::Orange,
            Measure::Space { .. } => Hue::Brown,
            Measure::Network { .. } => Hue::Teal,
            Measure::Temperature { .. } => Hue::Red,
            Measure::Power => Hue::Yellow,
        }
    }

    /// One of each kind, as Preferences' Add menu offers them.
    pub fn kinds() -> Vec<Measure> {
        vec![
            Measure::Cpu { cores: false },
            Measure::Memory { swap: false },
            Measure::Gpu { pci: None },
            Measure::Disk { device: None, read: true, write: true },
            Measure::Space { mount: "/".into() },
            Measure::Network { interface: None, down: true, up: true },
            Measure::Temperature { sensor: None },
            Measure::Power,
        ]
    }
}

/// Where on the Deskbar a widget is.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    /// An icon's place in the tray, beside the status icons (as BeOS's
    /// ProcessController and Pulse were).
    Tray,
    /// A row of its own under the tray.
    #[default]
    Top,
    /// A row of its own at the Deskbar's foot, under the applications.
    Foot,
}

impl Place {
    pub const ALL: [Place; 3] = [Place::Tray, Place::Top, Place::Foot];

    pub fn name(self) -> &'static str {
        match self {
            Place::Tray => "In the Tray",
            Place::Top => "Under the Tray",
            Place::Foot => "At the Bottom",
        }
    }
}

/// How a widget draws what it measures.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Look {
    /// The readings over time, filled under a line (each series its own
    /// line).
    #[default]
    Graph,
    /// A bar for each series, as Pulse's were for each processor.
    Bars,
    /// One level, across.
    Meter,
    /// The value alone, in words.
    Number,
    /// A cell for each series (each core), lit by its level: a heatmap.
    Heatmap,
}

impl Look {
    pub const ALL: [Look; 5] = [Look::Graph, Look::Bars, Look::Heatmap, Look::Meter, Look::Number];

    pub fn name(self) -> &'static str {
        match self {
            Look::Graph => "Graph",
            Look::Bars => "Bars",
            Look::Meter => "Meter",
            Look::Number => "Number",
            Look::Heatmap => "Heatmap",
        }
    }
}

/// How tall a widget's row is.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Size {
    /// Its words and its drawing on one line.
    Small,
    /// Its words over its drawing.
    #[default]
    Medium,
    /// The same, its drawing taller.
    Large,
}

impl Size {
    pub const ALL: [Size; 3] = [Size::Small, Size::Medium, Size::Large];

    pub fn name(self) -> &'static str {
        match self {
            Size::Small => "Small",
            Size::Medium => "Medium",
            Size::Large => "Large",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Settings from before widgets, or from an agent naming only what it
    /// wants, read whole.
    #[test]
    fn widgets_read_whatever_is_left_out() {
        let s: DeskbarSettings = serde_json::from_str("{}").unwrap();
        assert!(s.widgets.is_empty());
        let w: Widget = serde_json::from_str(r#"{"id": 3, "measure": {"kind": "network", "interface": null, "down": true, "up": false}, "place": "tray"}"#).unwrap();
        assert_eq!((w.id, w.place, w.every_ms, w.look), (3, Place::Tray, 1000, Look::Graph));
        assert_eq!(w.measure, Measure::Network { interface: None, down: true, up: false });
        let s = DeskbarSettings { widgets: vec![w.clone(), Widget::new(7, Measure::Power)] };
        assert_eq!(s.next_id(), 8);
        assert_eq!(serde_json::from_str::<DeskbarSettings>(&serde_json::to_string(&s).unwrap()).unwrap(), s);
    }

    /// A graph keeps as many readings as its span holds at its rate.
    #[test]
    fn a_graphs_history() {
        let mut w = Widget::new(1, Measure::Cpu { cores: false });
        assert_eq!(w.history(), 60);
        w.every_ms = 500;
        w.span_s = 30;
        assert_eq!(w.history(), 60);
        w.every_ms = 10;
        assert_eq!(w.every().as_millis(), MIN_EVERY_MS as u128);
    }
}
