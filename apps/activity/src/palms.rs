//! What the palm model makes of the touchpad's touches, live, for the
//! Touchpad card: Olo's touch filter (a libinput plugin) gives nothing
//! back but its log, so while the Inputs section shows, Activity follows
//! the pad's contacts as app_server reads them (`TouchContacts`, read
//! only while someone asks) and judges them as the filter does: the same
//! model (`olo_palm`, which the plugin agrees with), the same size rule.
//! Nothing runs while the section is hidden.

use std::{
    collections::{HashMap, VecDeque},
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use olo_protocol::{
    Client, Event, EventKind, Reply, Request,
    touchpad::{PalmModel, TouchFilter},
};

/// A minute of the model's judging.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PalmStats {
    /// Touches judged, and how many were palms (by their size alone
    /// among them).
    pub touches: u64,
    pub palms: u64,
    pub by_size: u64,
    /// The model's time a report (µs): typically, and the longest.
    pub us_p50: u64,
    pub us_max: u64,
    /// The model in use: none (size alone), or the person's / Olo's.
    pub model: bool,
}

/// The model the filter uses (app_server's `touch_filter::model_in_use`).
fn model_in_use(f: &TouchFilter) -> Option<PalmModel> {
    if !f.use_palm_model {
        return None;
    }
    match &f.palm_model {
        Some(m) if m.w1.len() == olo_palm::HIDDEN * olo_palm::FEATURES && m.mean.len() == olo_palm::FEATURES => Some(m.clone()),
        _ => Some(olo_palm::default_model()),
    }
}

/// Watching, while the Inputs section shows: the stream to shut to stop.
#[derive(Default, Clone)]
pub struct Watch(Arc<Mutex<Option<UnixStream>>>);

impl Watch {
    /// Start (`on`) or stop following the touches.
    pub fn want(&self, on: bool, send: impl Fn(PalmStats) + Send + 'static) {
        let mut open = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match (on, open.is_some()) {
            (true, false) => {
                let Ok(mut client) = Client::connect() else { return };
                let filter = match client.request(&Request::GetSettings) {
                    Ok(Reply::Settings { settings }) => settings.touchpad.filter,
                    _ => TouchFilter::default(),
                };
                let Ok(closer) = client.closer() else { return };
                *open = Some(closer);
                let watch = self.0.clone();
                let _ = std::thread::Builder::new().name("activity-palms".into()).spawn(move || {
                    follow(client, filter, send);
                    *watch.lock().unwrap_or_else(|e| e.into_inner()) = None;
                });
            }
            (false, true) => {
                if let Some(s) = open.take() {
                    let _ = s.shutdown(std::net::Shutdown::Both);
                }
            }
            _ => {}
        }
    }
}

/// One touch's fate.
struct Judged {
    at: Instant,
    palm: bool,
    by_size: bool,
}

fn follow(client: Client, filter: TouchFilter, send: impl Fn(PalmStats)) {
    let model = model_in_use(&filter);
    let Ok(events) = client.into_events_of(&[EventKind::TouchContacts]) else { return };
    let mut tracker = olo_palm::Tracker::default();
    // Each touch on the glass: whether it's been taken for a palm.
    let mut touching: HashMap<i32, (bool, bool)> = HashMap::new();
    let mut judged: VecDeque<Judged> = VecDeque::new();
    let mut times: VecDeque<(Instant, u64)> = VecDeque::new();
    let mut last_sent = Instant::now() - Duration::from_secs(2);
    for event in events {
        let Ok(Event::TouchContacts { pad, time_us, contacts, since_key_us }) = event else { break };
        let started = Instant::now();
        tracker.since_key_us = since_key_us;
        let frame = tracker.frame(&pad, time_us, &contacts);
        for (id, f, age_ms) in &frame {
            let e = touching.entry(*id).or_insert((false, false));
            if e.0 {
                continue;
            }
            let size = contacts.iter().find(|c| c.id == *id).map_or(0, |c| c.major.max(c.minor));
            if filter.palm_size > 0 && size >= filter.palm_size as i32 {
                *e = (true, true);
            } else if let Some(m) = &model
                && *age_ms >= olo_palm::MIN_AGE_MS
                && *age_ms <= olo_palm::DECIDE_MS
                && olo_palm::score(m, f) > m.threshold
            {
                *e = (true, false);
            }
        }
        let took = started.elapsed().as_micros() as u64;
        let now = Instant::now();
        // Lifted: counted.
        let before = judged.len();
        touching.retain(|id, (palm, by_size)| {
            let on = contacts.iter().any(|c| c.id == *id);
            if !on {
                judged.push_back(Judged { at: now, palm: *palm, by_size: *by_size });
            }
            on
        });
        times.push_back((now, took));
        let minute = Duration::from_secs(60);
        while judged.front().is_some_and(|j| now - j.at > minute) {
            judged.pop_front();
        }
        while times.front().is_some_and(|(t, _)| now - *t > minute) {
            times.pop_front();
        }
        // (Said as a touch lifts, else at most once a second.)
        if judged.len() != before || now - last_sent >= Duration::from_secs(1) {
            last_sent = now;
            let mut us: Vec<u64> = times.iter().map(|(_, u)| *u).collect();
            us.sort_unstable();
            send(PalmStats {
                touches: judged.len() as u64,
                palms: judged.iter().filter(|j| j.palm).count() as u64,
                by_size: judged.iter().filter(|j| j.by_size).count() as u64,
                us_p50: us.get(us.len() / 2).copied().unwrap_or(0),
                us_max: us.last().copied().unwrap_or(0),
                model: model.is_some(),
            });
        }
    }
}
