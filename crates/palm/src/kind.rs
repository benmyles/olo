//! The kinds of touchpad Olo has a palm model for. Each is its own model
//! type: features made from what that pad reports, a network of its own
//! size, a base model trained on real hands on that pad, when a contact is
//! judged, and how the trainer judges a try (a palm by its size only where
//! the pad reports one; a press by pressure or by the pad's button).
//!
//! - [`Kind::MacBook`]: Apple's pads (and any that report contact sizes):
//!   each contact's size and shape, pressure, place, speed, age and the
//!   others down ([`FEATURES`]); a hovering contact (no pressure) isn't
//!   on the glass yet.
//! - [`Kind::ThinkPad`]: the ThinkPad P1 Gen 9's Goodix GXTP5420
//!   (27C6:0F90), and any pad that reports no contact size. Measured on
//!   Ben's hands (`data/thinkpad-2026-11-05.json`, docs/TOUCHPAD.md): its
//!   firmware marks palms itself (the HID report's Confidence bit, which
//!   the kernel reports as `MT_TOOL_PALM`), from a palm's first report, a
//!   flat palm, a heel, a flat thumb, palms resting while typing alike;
//!   never a fingertip. But a resting palm comes back as a finger on a
//!   fifth to two fifths of its reports, and libinput follows the flag
//!   both ways, so a palm counts as a finger then. Its pressure is the
//!   haptic click's force, not the contact's area: a palm laid flat
//!   weighs 0, palms resting while typing 18–75 of 2000, a fingertip
//!   pressing up to 22 (libinput's quirks make it a pressure pad, which
//!   drops pressure; the touch filter's quirk lets it through, with
//!   libinput still not counting touches by it). Every contact it
//!   reports is on the glass (it reports no hover). The kernel ends a palm
//!   right after reporting it and it comes back at the same place under
//!   a new id, sometimes a finger from then on (a heel resting while a
//!   finger points): the tracker keeps it one contact. So its features
//!   are what the firmware has said of the contact (now, ever, how often,
//!   at once), how it moves and presses, and its age, not where it is;
//!   it is judged from its first report.

use olo_protocol::touchpad::{PadAxes, PalmModel, TouchContact};

use crate::FEATURES;

/// A kind of touchpad, with a palm model of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    MacBook,
    ThinkPad,
}

/// The firmware's word for a palm (`ABS_MT_TOOL_TYPE`).
pub const MT_TOOL_PALM: i32 = 2;

/// How many features a ThinkPad's contact has.
pub const THINKPAD_FEATURES: usize = 8;
/// A ThinkPad model's hidden layer: small, as what tells a palm is
/// mostly said outright (the firmware's flag), and small data can't fill
/// a large one.
pub const THINKPAD_HIDDEN: usize = 6;

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::MacBook, Kind::ThinkPad];

    /// The kind of the pad with `axes`: by its name, else by what it
    /// reports (a pad with no contact size is a ThinkPad's kind).
    pub fn of(axes: &PadAxes) -> Kind {
        let n = &axes.name;
        if n.contains("GXTP5420") || n.contains("27C6:0F90") {
            Kind::ThinkPad
        } else if n.contains("Apple") || !axes.size_from_pressure {
            Kind::MacBook
        } else {
            Kind::ThinkPad
        }
    }

    /// The kind a model is for (its `kind`; "" from before there were
    /// kinds: a MacBook's).
    pub fn of_model(m: &PalmModel) -> Kind {
        Kind::from_id(&m.kind)
    }

    pub fn from_id(id: &str) -> Kind {
        match id {
            "thinkpad" => Kind::ThinkPad,
            _ => Kind::MacBook,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Kind::MacBook => "macbook",
            Kind::ThinkPad => "thinkpad",
        }
    }

    /// What Preferences calls it.
    pub fn name(self) -> &'static str {
        match self {
            Kind::MacBook => "MacBook trackpad",
            Kind::ThinkPad => "ThinkPad touchpad",
        }
    }

    /// How many features its contacts have (the rest of a frame's
    /// [`FEATURES`] slots are 0).
    pub fn features(self) -> usize {
        match self {
            Kind::MacBook => FEATURES,
            Kind::ThinkPad => THINKPAD_FEATURES,
        }
    }

    /// Whether its pad reports each contact's size (so the size rule
    /// says palms too, and the trainer can draw a contact as large as it
    /// is).
    pub fn sized(self) -> bool {
        self == Kind::MacBook
    }

    /// Whether a click is the contacts' pressure (a MacBook's driver makes
    /// it so) or the pad's button (a ThinkPad's haptic click).
    pub fn clicks_by_pressure(self) -> bool {
        self == Kind::MacBook
    }

    /// Whether its firmware says which contacts are palms, so a palm it
    /// names stays one until it lifts.
    pub fn firmware_palms(self) -> bool {
        self == Kind::ThinkPad
    }

    /// Whether contact `c` is on the glass (not hovering above it).
    pub(crate) fn touching(self, axes: &PadAxes, c: &TouchContact) -> bool {
        match self {
            Kind::MacBook => axes.pressure_max <= 0 || c.pressure > 0,
            // (It reports no hover: a palm laid flat weighs nothing.)
            Kind::ThinkPad => true,
        }
    }

    /// Olo's own model for it: what runs until the person trains on this
    /// pad, and where their training starts.
    pub fn base_model(self) -> PalmModel {
        match self {
            Kind::MacBook => crate::default_model(),
            Kind::ThinkPad => thinkpad_model(),
        }
    }

    /// Its hand-written model, before any hands (training's very start).
    pub fn baseline(self) -> PalmModel {
        match self {
            Kind::MacBook => crate::baseline(),
            Kind::ThinkPad => thinkpad_baseline(),
        }
    }

    /// The model to run on it: `person`'s if it was trained on this kind
    /// of pad and has a model's shape, else the kind's own.
    pub fn model(self, person: Option<&PalmModel>) -> PalmModel {
        match person {
            Some(m) if Kind::of_model(m) == self && crate::fits(m) => m.clone(),
            _ => self.base_model(),
        }
    }
}

/// What the ThinkPad's tracker keeps of a contact beyond the shared
/// [`crate::Track`]: what the firmware has said of it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Said {
    pub reports: u32,
    pub palm_reports: u32,
    pub palm_first: bool,
    pub peak_pressure: f32,
}

/// A ThinkPad contact's features: what the firmware says of it (now,
/// ever, how often, at its first report), how it moves and presses (its
/// speed, its force now and at most), and its age. Not where it is: on
/// this pad thumbs waiting to click and heels land in the same corners,
/// and only the firmware tells them apart (a model that read the place
/// took fingertips tapping in the bottom corners for palms).
pub(crate) fn thinkpad_features(said: &mut Said, c: &TouchContact, speed: f32, pressure: f32, age_ms: u32) -> [f32; FEATURES] {
    let palm = c.tool == MT_TOOL_PALM;
    if said.reports == 0 {
        said.palm_first = palm;
    }
    said.reports += 1;
    said.palm_reports += palm as u32;
    said.peak_pressure = said.peak_pressure.max(pressure);
    let mut f = [0.0f32; FEATURES];
    f[..THINKPAD_FEATURES].copy_from_slice(&[
        palm as u32 as f32,
        (said.palm_reports > 0) as u32 as f32,
        said.palm_reports as f32 / said.reports as f32,
        said.palm_first as u32 as f32,
        speed.min(5.0) / 5.0,
        pressure,
        said.peak_pressure,
        (age_ms as f32 / 1000.0).min(1.0),
    ]);
    f
}

/// The ThinkPad's hand-written model: each hidden unit one sign, in the
/// features' own scale (`mean`, `std`). The firmware's palm, ever and now
/// (or at once), is most of it; a contact it never called a palm that
/// moves, or presses as a click does, takes away.
pub fn thinkpad_baseline() -> PalmModel {
    let (n, h) = (THINKPAD_FEATURES, THINKPAD_HIDDEN);
    let mean = vec![0.3, 0.3, 0.3, 0.3, 0.05, 0.01, 0.03, 0.15];
    let std = vec![0.45, 0.45, 0.45, 0.45, 0.1, 0.03, 0.05, 0.2];
    let mut w1 = vec![0.0f32; h * n];
    let mut b1 = vec![0.0f32; h];
    let mut set = |j: usize, b: f32, w: &[(usize, f32)]| {
        b1[j] = b;
        for &(i, v) in w {
            w1[j * n + i] = v;
        }
    };
    // 0: the firmware has called it a palm (and often).
    set(0, 0.0, &[(1, 1.0), (2, 0.4)]);
    // 1: the firmware calls it a palm now, or did at once.
    set(1, -0.3, &[(0, 0.7), (3, 0.7)]);
    // 2: never called a palm, and moving (a finger pointing): against.
    set(2, 0.0, &[(1, -0.8), (4, 0.8)]);
    // 3: never called a palm, pressing as a click does: against.
    set(3, 0.0, &[(1, -0.8), (6, 0.8)]);
    // 4: never called a palm at all, however still: against.
    set(4, 0.0, &[(1, -1.0), (2, -0.3)]);
    // 5: spare, for training to fill.
    set(5, 0.0, &[(7, 0.15), (5, -0.15)]);
    let w2 = vec![2.0, 1.2, -1.0, -0.8, -1.0, 0.0];
    PalmModel { kind: "thinkpad".into(), mean, std, w1, b1, w2, b2: -0.5, threshold: 0.5, decide_ms: crate::DECIDE_MS, min_age_ms: 0, ..Default::default() }
}

mod thinkpad_model;

/// The ThinkPad's model: [`thinkpad_baseline`] trained on Ben's hands on
/// the pad (`data/thinkpad-2026-11-05.json`), written into
/// `thinkpad_model.rs` by `make_thinkpad_model` (with how it did).
pub fn thinkpad_model() -> PalmModel {
    use thinkpad_model as d;
    let base = thinkpad_baseline();
    if d::MEAN.len() != THINKPAD_FEATURES || d::W1.len() != THINKPAD_HIDDEN * THINKPAD_FEATURES {
        return base;
    }
    PalmModel {
        kind: "thinkpad".into(),
        mean: d::MEAN.to_vec(),
        std: d::STD.to_vec(),
        lo: d::LO.to_vec(),
        hi: d::HI.to_vec(),
        w1: d::W1.to_vec(),
        b1: d::B1.to_vec(),
        w2: d::W2.to_vec(),
        b2: d::B2,
        threshold: d::THRESHOLD,
        decide_ms: base.decide_ms,
        min_age_ms: base.min_age_ms,
        trained: 0,
        contacts: d::CONTACTS,
        palms_caught: d::PALMS_CAUGHT,
        fingers_lost: d::FINGERS_LOST,
    }
}

/// A recording of hands on a pad, as kept in `data/`: each report as the
/// kernel's slots stood after it, and what each step of it was.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Capture {
    pub pad: PadAxes,
    pub steps: Vec<Step>,
    /// Contacts (tracking ids) that were palms whatever their step was (a
    /// heel resting while a finger points).
    #[serde(default)]
    pub palms: Vec<i32>,
    /// (microseconds, the button, contacts as [id, x, y, pressure, tool]).
    pub reports: Vec<(u64, i32, Vec<[i32; 5]>)>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Step {
    pub from_us: u64,
    pub to_us: u64,
    pub palm: bool,
    pub what: String,
}

impl Capture {
    pub fn parse(bytes: &[u8]) -> Option<Capture> {
        serde_json::from_slice(bytes).ok()
    }

    /// Its contacts as the trainer would keep them: each one's features
    /// from its first report on the glass while the model could judge it
    /// (`model`'s `min_age_ms` to `decide_ms`), labelled by the step it
    /// landed in (one landing in none is left out).
    pub fn labelled(&self, model: &PalmModel) -> Vec<crate::Labelled> {
        let mut tracker = crate::Tracker::default();
        let mut open: std::collections::HashMap<i32, (crate::Labelled, u64)> = std::collections::HashMap::new();
        let mut done = Vec::new();
        for (time_us, _, cs) in &self.reports {
            let contacts: Vec<TouchContact> = cs.iter().map(|&[id, x, y, pressure, tool]| TouchContact { id, x, y, pressure, tool, ..Default::default() }).collect();
            for gone in open.keys().copied().filter(|id| !contacts.iter().any(|c| c.id == *id)).collect::<Vec<_>>() {
                done.push(open.remove(&gone).expect("open"));
            }
            for (id, f, age_ms) in tracker.frame(&self.pad, *time_us, &contacts) {
                let step = self.steps.iter().find(|s| (s.from_us..s.to_us).contains(time_us));
                let palm = step.is_some_and(|s| s.palm) || self.palms.contains(&id);
                let (c, _) = open.entry(id).or_insert_with(|| (crate::Labelled { frames: Vec::new(), palm, size: 0 }, *time_us));
                if age_ms >= model.min_age_ms && age_ms <= model.decide_ms {
                    c.frames.push(f);
                }
            }
        }
        done.extend(open.into_values());
        done.sort_by_key(|(_, at)| *at);
        done.into_iter().filter(|(c, at)| !c.frames.is_empty() && self.steps.iter().any(|s| (s.from_us..s.to_us).contains(at))).map(|(c, _)| c).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Labelled, is_palm, judge, score, train_from};

    /// Ben's hands on the ThinkPad's pad, 2026-11-05: palms (and a few
    /// fingertips), then fingers (and a heel resting while one points).
    fn bens() -> Capture {
        Capture::parse(include_bytes!("../data/thinkpad-2026-11-05.json")).expect("the capture")
    }

    fn captures() -> Vec<Capture> {
        vec![bens(), Capture::parse(include_bytes!("../data/thinkpad-2026-11-05-fingers.json")).expect("the capture")]
    }

    fn labelled(m: &PalmModel) -> Vec<Labelled> {
        captures().iter().flat_map(|c| c.labelled(m)).collect()
    }

    fn line(name: &str, m: &PalmModel, cs: &[Labelled]) -> String {
        let r = judge(m, &cs.iter().collect::<Vec<_>>(), 0);
        format!("{name}: {} palms, {} fingers; caught {:.1}% of palms, took {:.1}% of fingers", r.palms, r.fingers, r.palms_caught * 100.0, r.fingers_lost * 100.0)
    }

    #[test]
    fn a_thinkpad_is_its_own_kind() {
        let pad = bens().pad;
        assert_eq!(Kind::of(&pad), Kind::ThinkPad);
        assert_eq!(Kind::of(&PadAxes::macbook()), Kind::MacBook);
        for k in Kind::ALL {
            let m = k.base_model();
            assert!(crate::fits(&m), "{k:?}'s model has its shape");
            assert_eq!(Kind::of_model(&m), k);
            assert_eq!(Kind::from_id(k.id()), k);
        }
        // A model from before there were kinds is a MacBook's; a person's
        // model runs only on its own kind of pad.
        assert_eq!(Kind::of_model(&PalmModel::default()), Kind::MacBook);
        let mine = PalmModel { trained: 7, ..crate::default_model() };
        assert_eq!(Kind::MacBook.model(Some(&mine)).trained, 7);
        assert_eq!(Kind::ThinkPad.model(Some(&mine)).kind, "thinkpad");
    }

    #[test]
    fn the_thinkpad_model_on_bens_hands() {
        let cap = bens();
        let m = thinkpad_model();
        let cs = labelled(&m);
        let (palms, fingers) = (cs.iter().filter(|c| c.palm).count(), cs.iter().filter(|c| !c.palm).count());
        assert!(palms >= 20 && fingers >= 50, "{palms} palms, {fingers} fingers");
        let r = judge(&m, &cs.iter().collect::<Vec<_>>(), 0);
        assert_eq!((r.palms_caught, r.fingers_lost), (1.0, 0.0), "{}", line("the ThinkPad's model", &m, &cs));
        // Each palm within a few reports (its firmware says so at once, or
        // nearly: 28 ms at most on Ben's hands), none waiting for an age.
        let slowest = cs.iter().filter(|c| c.palm).map(|c| c.frames.iter().position(|f| score(&m, f) > m.threshold).expect("caught")).max().unwrap_or(0);
        assert!(slowest <= 5, "a palm caught only at its report {slowest}");
        // The MacBook's model, which has no size here to go by, does worse.
        let mac = crate::default_model();
        let theirs: Vec<Labelled> =
            captures().into_iter().flat_map(|c| Capture { pad: PadAxes { name: "Apple MTP".into(), size_from_pressure: false, ..cap.pad.clone() }, ..c }.labelled(&mac)).collect();
        let r2 = judge(&mac, &theirs.iter().collect::<Vec<_>>(), 0);
        assert!(r2.palms_caught < 1.0 || r2.fingers_lost > 0.0, "{}", line("the MacBook's model", &mac, &theirs));
    }

    /// A palm the firmware calls a finger again later is still caught
    /// (it said palm at once), and a finger it never calls a palm isn't.
    #[test]
    fn the_firmware_flag_is_what_counts_most() {
        let m = thinkpad_model();
        let pad = bens().pad;
        let mut t = crate::Tracker::default();
        let mut palm_scores = Vec::new();
        for i in 0..20u64 {
            let tool = if i % 3 == 1 { 0 } else { MT_TOOL_PALM };
            let c = TouchContact { id: 1, x: 150, y: 1000, pressure: 30, tool, ..Default::default() };
            for (_, f, _) in t.frame(&pad, i * 7000, &[c]) {
                palm_scores.push(score(&m, &f));
            }
        }
        assert!(palm_scores.iter().all(|s| *s > m.threshold), "{palm_scores:?} vs {}", m.threshold);
        let mut t = crate::Tracker::default();
        for i in 0..30u64 {
            let c = TouchContact { id: 2, x: 1800 + i as i32 * 20, y: 800, pressure: 8, tool: 0, ..Default::default() };
            for (_, f, _) in t.frame(&pad, i * 7000, &[c]) {
                assert!(score(&m, &f) < m.threshold, "a finger pointing taken for a palm");
            }
        }
    }

    /// The ThinkPad's model made anew from the captures in `data/`:
    /// `thinkpad_baseline` trained on them, written into
    /// `kind/thinkpad_model.rs` with how it did: `cargo test --release -p
    /// olo-palm make_thinkpad_model -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn make_thinkpad_model() {
        let base = thinkpad_baseline();
        let cs = labelled(&base);
        // (No size here: the size rule says nothing.)
        let (m, r) = train_from(&base, &cs, u32::MAX, |_| {}).expect("enough to train on");
        let all = judge(&m, &cs.iter().collect::<Vec<_>>(), 0);
        println!("{} contacts; kept back: {r:?}", cs.len());
        println!("  {}", line("trained", &m, &cs));
        println!("  {}", line("hand-written", &base, &cs));
        let missed: Vec<usize> = cs.iter().enumerate().filter(|(_, c)| c.palm != is_palm(&m, c)).map(|(i, _)| i).collect();
        for &i in &missed {
            let c = &cs[i];
            let f = c.frames[0];
            println!("  wrong: #{i} palm {} frames {} first: {:?}; best score {:.2}", c.palm, c.frames.len(), &f[..THINKPAD_FEATURES], c.frames.iter().map(|f| score(&m, f)).fold(f32::MIN, f32::max));
        }
        assert!(missed.is_empty(), "wrong on {missed:?}");
        let list = |v: &[f32]| v.iter().map(|x| format!("{x:?}")).collect::<Vec<_>>().join(", ");
        let text = format!(
            "//! The ThinkPad's palm model (`thinkpad_model`): `thinkpad_baseline`\n\
             //! trained on Ben's hands on the pad (`data/thinkpad-2026-11-05*.json`:\n\
             //! a flat palm, a heel, a flat thumb, palms resting as when typing;\n\
             //! fingertips pointing to every edge and corner, tapping, clicking,\n\
             //! dragging and scrolling, a thumb resting low and clicking, a heel\n\
             //! resting while a finger points), by `make_thinkpad_model` (don't\n\
             //! edit: run it again). {} contacts ({} palms, {} fingers): it catches {:.1}% of\n\
             //! the palms, each by its report {} (the firmware's word, at once or\n\
             //! nearly), and takes {:.1}% of the fingers\n\
             //! for palms (on the {} kept back from training: {:.1}% and {:.1}%; the\n\
             //! hand-written one there: {:.1}% and {:.1}%).\n\n\
             pub(crate) const CONTACTS: u32 = {};\n\
             pub(crate) const PALMS_CAUGHT: f32 = {:?};\n\
             pub(crate) const FINGERS_LOST: f32 = {:?};\n\
             pub(crate) const THRESHOLD: f32 = {:?};\n\
             pub(crate) const B2: f32 = {:?};\n\
             pub(crate) const MEAN: &[f32] = &[{}];\n\
             pub(crate) const STD: &[f32] = &[{}];\n\
             pub(crate) const LO: &[f32] = &[{}];\n\
             pub(crate) const HI: &[f32] = &[{}];\n\
             pub(crate) const B1: &[f32] = &[{}];\n\
             pub(crate) const W2: &[f32] = &[{}];\n\
             pub(crate) const W1: &[f32] = &[{}];\n",
            cs.len(),
            all.palms,
            all.fingers,
            all.palms_caught * 100.0,
            cs.iter().filter(|c| c.palm).filter_map(|c| c.frames.iter().position(|f| score(&m, f) > m.threshold)).max().unwrap_or(0) + 1,
            all.fingers_lost * 100.0,
            r.contacts,
            r.palms_caught * 100.0,
            r.fingers_lost * 100.0,
            r.baseline_palms_caught * 100.0,
            r.baseline_fingers_lost * 100.0,
            cs.len(),
            all.palms_caught,
            all.fingers_lost,
            m.threshold,
            m.b2,
            list(&m.mean),
            list(&m.std),
            list(&m.lo),
            list(&m.hi),
            list(&m.b1),
            list(&m.w2),
            list(&m.w1),
        );
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/kind/thinkpad_model.rs");
        std::fs::write(&path, text).unwrap();
        println!("written: {}", path.display());
    }
}

#[cfg(test)]
mod unseen {
    use super::*;

    /// Fingers as the captures don't have them (still for a long while
    /// mid-pad, a thumb resting low, at either edge with another down, two
    /// scrolling, pressing to click) stay fingers; a palm the firmware
    /// called a palm once and a finger after stays a palm.
    #[test]
    fn fingers_it_never_saw_stay_fingers() {
        let m = thinkpad_model();
        let pad = Capture::parse(include_bytes!("../data/thinkpad-2026-11-05.json")).unwrap().pad;
        let one = |x, y, p| move |_: i32| vec![TouchContact { id: 1, x, y, pressure: p, ..Default::default() }];
        let cases: Vec<(&str, bool, Box<dyn Fn(i32) -> Vec<TouchContact>>)> = vec![
            ("a fingertip still mid-pad", false, Box::new(one(1800, 900, 5))),
            ("a thumb resting low to click", false, Box::new(one(1500, 1700, 15))),
            ("a fingertip still at the left edge", false, Box::new(one(120, 900, 5))),
            ("a fingertip still in a bottom corner", false, Box::new(one(3480, 1760, 30))),
            (
                "a fingertip at the right edge, another down",
                false,
                Box::new(|_| vec![TouchContact { id: 1, x: 3480, y: 900, pressure: 20, ..Default::default() }, TouchContact { id: 2, x: 1800, y: 900, pressure: 5, ..Default::default() }]),
            ),
            (
                "two fingers scrolling",
                false,
                Box::new(|i| {
                    vec![TouchContact { id: 1, x: 1700, y: 600 + i * 10, pressure: 6, ..Default::default() }, TouchContact { id: 2, x: 2000, y: 600 + i * 10, pressure: 6, ..Default::default() }]
                }),
            ),
            ("a fingertip pressing to click", false, Box::new(one(1800, 1200, 340))),
            (
                "a heel called a palm once, a finger after",
                true,
                Box::new(|i| vec![TouchContact { id: 1, x: 3580, y: 1704, pressure: 30, tool: if i == 0 { MT_TOOL_PALM } else { 0 }, ..Default::default() }]),
            ),
        ];
        for (name, palm, frames) in cases {
            let mut t = crate::Tracker::default();
            let mut most = f32::MIN;
            for i in 0..57 {
                for (_, f, age) in t.frame(&pad, i as u64 * 7000, &frames(i)) {
                    if age >= m.min_age_ms && age <= m.decide_ms {
                        most = most.max(crate::score(&m, &f));
                    }
                }
            }
            assert_eq!(most > m.threshold, palm, "{name}: {most:.2} against {:.2}", m.threshold);
        }
    }

    /// The kernel ends a palm and it comes back under a new id at the same
    /// place, a finger from then on: still the palm it was.
    #[test]
    fn a_palm_back_under_a_new_id_is_still_a_palm() {
        let m = thinkpad_model();
        let pad = Capture::parse(include_bytes!("../data/thinkpad-2026-11-05.json")).unwrap().pad;
        let mut t = crate::Tracker::default();
        let at = |id, tool| vec![TouchContact { id, x: 3580, y: 1704, pressure: 40, tool, ..Default::default() }];
        let _ = t.frame(&pad, 0, &at(2357, MT_TOOL_PALM));
        let _ = t.frame(&pad, 7_000, &[]);
        let back = t.frame(&pad, 14_000, &at(2359, 0));
        assert!(crate::score(&m, &back[0].1) > m.threshold, "the heel back as a finger");
        // Not so far away, or so much later: another contact.
        let mut t = crate::Tracker::default();
        let _ = t.frame(&pad, 0, &at(1, MT_TOOL_PALM));
        let _ = t.frame(&pad, 7_000, &[]);
        let other = t.frame(&pad, 14_000, &[TouchContact { id: 2, x: 1800, y: 900, pressure: 5, ..Default::default() }]);
        assert!(crate::score(&m, &other[0].1) < m.threshold, "a finger elsewhere");
        let _ = t.frame(&pad, 100_000, &[]);
        let _ = t.frame(&pad, 107_000, &at(3, MT_TOOL_PALM));
        let _ = t.frame(&pad, 114_000, &[]);
        let late = t.frame(&pad, 200_000, &at(4, 0));
        assert!(crate::score(&m, &late[0].1) < m.threshold, "a finger landing later where a palm was");
    }
}
