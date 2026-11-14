//! The pool's meters for Deskbar's widgets and agents
//! (`olo_protocol::pool`): where each account's quotas stand, written by
//! the copy serving to a small file in the runtime directory as they
//! change (a request's answer, a usage report, an account turned off),
//! half a second after the first change of a burst, and only when what
//! it says differs. A copy that has handed over to a new version writes
//! nothing more: the new one does. Nothing here wakes while nothing
//! changes.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Weak, mpsc},
    time::Duration,
};

use olo_protocol::pool::{PoolAccount, PoolMeters, PoolWindow};

use crate::{
    gateway::{Pool, Shared},
    model::{Account, Provider},
};

/// How long a burst of changes is let run before it's written.
const SETTLE: Duration = Duration::from_millis(500);
/// How far back a window's pace looks.
const PACE_SPAN: i64 = 3600;
/// Where each window stood is noted at most this often (a change
/// meanwhile moves the latest note).
const NOTE_EVERY: i64 = 30;

/// Where each account's windows stood over the last hour, oldest first:
/// what their pace is worked out from.
#[derive(Default)]
pub struct Paces(HashMap<(String, String), VecDeque<(i64, f64)>>);

impl Paces {
    /// Note where each window stands at `now`. One that has reset since
    /// (less used than before, or its reset later) starts again.
    pub fn note(&mut self, accounts: &[Account], now: i64) {
        let mut seen = Vec::new();
        for a in accounts {
            for w in &a.windows {
                let key = (a.id.clone(), w.name.clone());
                let notes = self.0.entry(key.clone()).or_default();
                if notes.back().is_some_and(|(_, used)| w.used < *used) {
                    notes.clear();
                }
                // A note every NOTE_EVERY at most: a change sooner moves
                // the latest (never the oldest: where it stood then).
                match notes.back() {
                    Some((at, _)) if now - at < NOTE_EVERY => {
                        if notes.len() > 1
                            && let Some(last) = notes.back_mut()
                        {
                            last.1 = w.used;
                        }
                    }
                    _ => notes.push_back((now, w.used)),
                }
                while notes.len() > 1 && notes.front().is_some_and(|(at, _)| now - at > PACE_SPAN) {
                    notes.pop_front();
                }
                seen.push(key);
            }
        }
        // (An account or window gone: its notes too.)
        self.0.retain(|k, _| seen.contains(k));
    }

    /// The oldest note of a window within the hour: where it stood then.
    pub fn since(&self, account: &str, window: &str) -> Option<(i64, f64)> {
        self.0.get(&(account.to_owned(), window.to_owned()))?.front().copied()
    }
}

/// The pool as its meters say it: each account's quotas, and where they
/// stood as far back as an hour.
pub fn of(p: &Pool, paused: &[Provider], paces: &Paces, pid: u32) -> PoolMeters {
    let provider = |p: Provider| match p {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    };
    let accounts = p
        .saved
        .accounts
        .iter()
        .map(|a| PoolAccount {
            id: a.id.clone(),
            name: a.name.clone(),
            provider: provider(a.provider).into(),
            enabled: a.enabled,
            signed_out: a.signed_out.clone(),
            blocked_until: a.cooldown_until,
            model_blocked: a.model_cooldowns.clone(),
            in_flight: p.in_flight.get(&a.id).copied().unwrap_or(0),
            last_used: a.last_used,
            synced: a.synced,
            windows: a
                .windows
                .iter()
                .map(|w| PoolWindow { name: w.name.clone(), label: w.label.clone(), used: w.used, resets_at: w.resets_at, models: w.models.clone(), since: paces.since(&a.id, &w.name) })
                .collect(),
        })
        .collect();
    PoolMeters { pid, loaded: p.loaded, accounts, paused: paused.iter().map(|p| provider(*p).to_owned()).collect() }
}

/// The thread that writes them: each time `changes` says something
/// changed (and once as it starts), after `SETTLE`, if it says something
/// new. Ends with the pool.
pub fn spawn(shared: Weak<Shared>, changes: mpsc::Receiver<()>) {
    let _ = std::thread::Builder::new().name("pool-meters".into()).spawn(move || {
        let mut paces = Paces::default();
        let mut written: Option<Vec<u8>> = None;
        let mut first = true;
        loop {
            if !std::mem::take(&mut first) && changes.recv().is_err() {
                return;
            }
            std::thread::sleep(SETTLE);
            while changes.try_recv().is_ok() {}
            let Some(s) = shared.upgrade() else { return };
            // (Handed over: the new copy says how they stand.)
            if s.drain.retired() {
                continue;
            }
            let now = crate::model::now();
            let paused: Vec<Provider> = [Provider::Claude, Provider::Codex].into_iter().filter(|p| s.paused(*p)).collect();
            let meters = {
                let p = s.pool.lock().unwrap();
                paces.note(&p.saved.accounts, now);
                of(&p, &paused, &paces, std::process::id())
            };
            drop(s);
            let Ok(bytes) = serde_json::to_vec(&meters) else { continue };
            if written.as_ref() != Some(&bytes) {
                match meters.save() {
                    Ok(()) => written = Some(bytes),
                    Err(e) => tracing::debug!("pool: couldn’t write its meters: {e}"),
                }
            }
        }
    });
}

/// `changed`, and the meters' writer told too.
pub fn and_written(changed: impl Fn() + Send + Sync + 'static) -> (impl Fn() + Send + Sync + 'static, mpsc::Receiver<()>) {
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    (
        move || {
            changed();
            let _ = tx.lock().unwrap().send(());
        },
        rx,
    )
}

/// Started for the pool `shared`, told by `changes`.
pub fn start(shared: &Arc<Shared>, changes: mpsc::Receiver<()>) {
    spawn(Arc::downgrade(shared), changes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Window;

    fn account(used: f64) -> Account {
        let mut a = Account::new("work".into(), "Work".into(), Provider::Claude);
        a.windows = vec![Window { name: "five_hour".into(), label: "5-hour".into(), used, resets_at: Some(10_000), models: Vec::new() }];
        a
    }

    /// A window's pace from where it stood up to an hour before: notes
    /// at most every 30 s, a reset starting again, older ones let go.
    #[test]
    fn paces_from_the_last_hour() {
        let mut paces = Paces::default();
        let mut a = account(10.0);
        let id = a.id.clone();
        paces.note(std::slice::from_ref(&a), 0);
        a.windows[0].used = 12.0;
        paces.note(std::slice::from_ref(&a), 10);
        a.windows[0].used = 14.0;
        paces.note(std::slice::from_ref(&a), 20);
        assert_eq!(paces.since(&id, "five_hour"), Some((0, 10.0)));
        assert_eq!(paces.0[&(id.clone(), "five_hour".to_owned())].len(), 1);
        a.windows[0].used = 20.0;
        paces.note(std::slice::from_ref(&a), 40);
        a.windows[0].used = 22.0;
        paces.note(std::slice::from_ref(&a), 50);
        assert_eq!(paces.0[&(id.clone(), "five_hour".to_owned())], VecDeque::from([(0, 10.0), (40, 22.0)]));
        // Over an hour on, the notes before it go.
        a.windows[0].used = 40.0;
        paces.note(std::slice::from_ref(&a), 4_000);
        assert_eq!(paces.since(&id, "five_hour"), Some((4_000, 40.0)));
        // Reset: less used than before.
        a.windows[0].used = 1.0;
        paces.note(std::slice::from_ref(&a), 4_100);
        assert_eq!(paces.since(&id, "five_hour"), Some((4_100, 1.0)));
    }

    /// The meters say each account's windows and where they stood.
    #[test]
    fn meters_of_the_pool() {
        let mut p = Pool::default();
        let mut a = account(30.0);
        a.cooldown_until = Some(500);
        p.saved.accounts.push(a.clone());
        p.in_flight.insert(a.id.clone(), 2);
        let mut paces = Paces::default();
        paces.note(&p.saved.accounts, 100);
        let m = of(&p, &[Provider::Codex], &paces, 42);
        assert_eq!((m.pid, m.paused.clone()), (42, vec!["codex".to_owned()]));
        let got = &m.accounts[0];
        assert_eq!((got.provider.as_str(), got.in_flight, got.blocked_until), ("claude", 2, Some(500)));
        assert_eq!(got.windows[0].label, "5-hour");
        assert_eq!(got.windows[0].since, Some((100, 30.0)));
        // (What a widget makes of it.)
        let a = olo_protocol::pool::availability(&m, "claude", Some("5-hour"), false, 1_000).unwrap();
        assert!((a.left - 0.7).abs() < 1e-9);
    }
}
