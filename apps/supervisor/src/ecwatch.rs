//! Says so when the laptop's embedded controller floods the kernel with
//! events (`olo_system::ec_storm`): a notification that stays, and a
//! file Olo Doctor reads (`EcStorm::path`), for as long as it lasts.
//! Always on, whatever Preferences ▸ Performance says: it isn't about
//! the person's programs, and it costs nothing while the power supply
//! holds still. (Ben, 2026-11-17, his brightness keys dead for two hours
//! and the battery running down on its charger: "add something to olo's
//! supervisor to do this if an unsustainable backlog builds up".)

use olo_kit::notify::{Note, NoteEvent, NoteId, NoteKind, Notifier};
use olo_protocol::performance::EcStorm;
use olo_sync::Mutex;
use std::sync::Arc;

/// Follow the controller from now on.
pub fn start() {
    let notes = Notifier::with_events("Olo Doctor", "olo-doctor", |e| {
        if let NoteEvent::Action { .. } = e {
            let _ = olo_xdg::open::spawn_app(&["olo-doctor".into()], "olo.doctor");
        }
    });
    let shown: Arc<Mutex<Option<NoteId>>> = Default::default();
    let said = move |storm: Option<olo_system::ec_storm::Storm>| match storm {
        Some(s) => {
            let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let found = EcStorm { since, events_per_second: s.events_per_second, waiting: s.waiting };
            tracing::warn!("supervisor: the embedded controller is flooding the kernel: {:.0} events a second, {} handlers waiting", s.events_per_second, s.waiting);
            olo_kit::store::save_bytes(EcStorm::path(), serde_json::to_vec(&found).unwrap_or_default());
            let note = Note::new(EcStorm::title()).body(EcStorm::summary()).kind(NoteKind::Important).sticky(true).action("default", "Open Olo Doctor");
            let mut shown = shown.lock().unwrap();
            *shown = Some(notes.show(*shown, note));
        }
        None => {
            tracing::info!("supervisor: the embedded controller's flood is over");
            let _ = std::fs::remove_file(EcStorm::path());
            if let Some(id) = shown.lock().unwrap().take() {
                notes.close(id);
            }
        }
    };
    // (Left by a supervisor that ended in one: not true any more, until
    // measured again.)
    let _ = std::fs::remove_file(EcStorm::path());
    // `OLO_EC_STORM_DEMO=SECONDS`: a made-up storm said now and over
    // after that long, to see what the person would (a test session;
    // nothing is measured).
    if let Some(secs) = std::env::var("OLO_EC_STORM_DEMO").ok().and_then(|s| s.parse::<u64>().ok()) {
        let _ = std::thread::Builder::new().name("ec-storm-demo".into()).spawn(move || {
            said(Some(olo_system::ec_storm::Storm { events_per_second: 46.0, waiting: 352 }));
            std::thread::sleep(std::time::Duration::from_secs(secs));
            said(None);
        });
        return;
    }
    let following = olo_system::ec_storm::follow(said);
    if !following {
        tracing::info!("supervisor: no UPower to say when the power supply changes: the embedded controller isn't followed");
    }
}
