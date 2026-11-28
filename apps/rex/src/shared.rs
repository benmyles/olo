//! What Rex takes from the desktop's settings (`settings.rex`,
//! Preferences ▸ Rex): which changes of an agent become notifications,
//! the hosts it knows, and what the sessions of each may do on this
//! computer. Rex kept all three in a file and a Settings window of its
//! own as it came; they are the desktop's now, followed live, and Rex's
//! file keeps a window's own quick choices only.
//!
//! The app's code still reads its `Settings` (`saved_hosts`,
//! `host_policies`, the notification rules): [`apply`] makes those
//! three say what the desktop's settings say, and tells what changed.
//! [`carry_over`] is the one time the other way: what the person
//! already had in Rex's file, into the desktop's.

use std::collections::HashMap;

use olo_protocol::{RexGrants, RexHost, RexSettings};
use serde::{Deserialize, Serialize};

use crate::settings::{HostBridgePolicy, Notices, SavedHost, Settings};

/// The desktop's grants for a host as the app's own policy.
pub fn policy(g: RexGrants) -> HostBridgePolicy {
    HostBridgePolicy {
        allow_open_url: g.open_links,
        allow_localhost_autoforward: g.forward_ports,
        allow_agent_screens: g.agent_screens,
        allow_passwords_op: g.passwords,
        allow_remote_control: g.clipboard,
        allow_ssh_agent: g.ssh_keys,
        allow_send_files: g.send_files,
        allow_run_commands: g.run_commands,
    }
}

/// What Rex's own file held before these became the desktop's, read as
/// the app wrote it then: its hosts, each host's permissions (the five
/// that were acted on; two more it wrote were never read), and which
/// changes of an agent it notified.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Old {
    pub saved_hosts: Vec<SavedHost>,
    pub host_policies: HashMap<String, OldPolicy>,
    pub notifications: OldNotices,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OldPolicy {
    pub allow_open_url: bool,
    pub allow_localhost_autoforward: bool,
    pub allow_agent_screens: bool,
    pub allow_passwords_op: bool,
    pub allow_remote_control: bool,
}

impl OldPolicy {
    fn grants(self) -> RexGrants {
        RexGrants {
            open_links: self.allow_open_url,
            forward_ports: self.allow_localhost_autoforward,
            agent_screens: self.allow_agent_screens,
            passwords: self.allow_passwords_op,
            clipboard: self.allow_remote_control,
            // (Rex's own file wrote one for this and never acted on
            // it: not carried over as a yes.)
            ssh_keys: false,
            send_files: false,
            run_commands: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OldNotices {
    pub blocked: OldRule,
    pub error: OldRule,
    pub done: OldRule,
}

/// (Each of the three notified unless turned off.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OldRule {
    pub desktop_notification: bool,
}

impl Default for OldRule {
    fn default() -> Self {
        Self { desktop_notification: true }
    }
}

/// What following the desktop's settings changed in the app's
/// ([`apply`]): the hosts that are new (to be shown, and connected if
/// they are to be at start), those gone (to be let go of), those whose
/// name or address is another; and whether anything at all did.
#[derive(Debug, Default, PartialEq)]
pub struct Changes {
    pub added: Vec<SavedHost>,
    pub removed: Vec<String>,
    pub renamed: Vec<SavedHost>,
    pub moved: Vec<SavedHost>,
    pub any: bool,
}

/// Make the app's settings say what the desktop's do, and tell what
/// changed.
pub fn apply(rex: &RexSettings, s: &mut Settings) -> Changes {
    let mut changes = Changes::default();
    let before = s.clone_followed();
    let hosts: Vec<SavedHost> = rex
        .hosts
        .iter()
        // (One with no address yet is still being typed in Preferences.)
        .filter(|h| !h.id.is_empty() && !h.endpoint.trim().is_empty())
        .map(|h| SavedHost {
            id: h.id.clone(),
            label: if h.label.trim().is_empty() { h.endpoint.clone() } else { h.label.clone() },
            endpoint: h.endpoint.trim().to_owned(),
            icon: None,
            auto_connect: h.connect_at_start,
            ssh_agent_socket: None,
        })
        .collect();
    for host in &hosts {
        match s.saved_hosts.iter().find(|old| old.id == host.id) {
            None => changes.added.push(host.clone()),
            Some(old) if old.endpoint != host.endpoint => changes.moved.push(host.clone()),
            Some(old) if old.label != host.label => changes.renamed.push(host.clone()),
            Some(_) => {}
        }
    }
    changes.removed = s.saved_hosts.iter().filter(|old| !hosts.iter().any(|h| h.id == old.id)).map(|old| old.id.clone()).collect();
    s.saved_hosts = hosts;
    s.host_policies.clear();
    s.host_policies.insert("local".to_owned(), policy(rex.local));
    for h in &rex.hosts {
        s.host_policies.insert(h.id.clone(), policy(h.grants));
    }
    s.notices = Notices { blocked: rex.notify_blocked, done: rex.notify_done, failed: rex.notify_failed, minded_subagents: rex.notify_minded_subagents };
    changes.any = before != s.clone_followed();
    changes
}

/// Once: what the person already had in Rex's own file, into the
/// desktop's settings. Hosts by their ids (one the desktop already has
/// is left as it is there), each with the permissions it had (none it
/// didn't have); this computer's own permissions and the three notices
/// only where the desktop's still say what they come with. Whether
/// `rex` was changed.
pub fn carry_over(old: &Old, rex: &mut RexSettings) -> bool {
    let before = rex.clone();
    for host in &old.saved_hosts {
        if host.id.is_empty() || rex.hosts.iter().any(|h| h.id == host.id) {
            continue;
        }
        let granted = old.host_policies.get(&host.id).map(|p| p.grants()).unwrap_or_default();
        rex.hosts.push(RexHost { id: host.id.clone(), label: host.label.clone(), endpoint: host.endpoint.clone(), connect_at_start: host.auto_connect, grants: granted, moved_to: None });
    }
    let fresh = RexSettings::default();
    if rex.local == fresh.local
        && let Some(local) = old.host_policies.get("local")
    {
        rex.local = local.grants();
    }
    if (rex.notify_blocked, rex.notify_done, rex.notify_failed) == (fresh.notify_blocked, fresh.notify_done, fresh.notify_failed) {
        rex.notify_blocked = old.notifications.blocked.desktop_notification;
        rex.notify_done = old.notifications.done.desktop_notification;
        rex.notify_failed = old.notifications.error.desktop_notification;
    }
    *rex != before
}

/// The hosts being carried into the desktop's settings right now, and
/// until when to wait for them there ([`adopt`], [`settled`]).
static CARRYING: olo_sync::Mutex<Option<(Vec<String>, std::time::Instant)>> = olo_sync::Mutex::new(None);

/// The desktop's settings for Rex as they are to be followed, given
/// what Rex's own file still holds from before: the first time, that is
/// carried into them (here, and told to the desktop on a thread of its
/// own), and marked done in Rex's file.
pub fn adopt(s: &mut Settings, rex: RexSettings) -> RexSettings {
    adopt_telling(s, rex, |old| {
        olo_kit::settings::update(move |desktop| {
            carry_over(&old, &mut desktop.rex);
        });
    })
}

/// [`adopt`], with the telling of the desktop given (a test tells
/// nobody: run in a shell of a session, it would write the session's
/// settings).
fn adopt_telling(s: &mut Settings, mut rex: RexSettings, tell: impl FnOnce(Old)) -> RexSettings {
    let Some(old) = s.before_preferences.take() else { return rex };
    s.carried_to_preferences = true;
    if carry_over(&old, &mut rex) {
        *CARRYING.lock().unwrap_or_else(|e| e.into_inner()) = Some((old.saved_hosts.iter().map(|h| h.id.clone()).collect(), std::time::Instant::now() + std::time::Duration::from_secs(5)));
        tell(old);
    }
    rex
}

/// Whether the desktop's settings, as just told, are to be followed:
/// not while what [`adopt`] is carrying into them hasn't arrived there
/// (they would say the carried hosts are gone, and Rex would let go of
/// hosts it is connecting to, only to take them up again a moment
/// later). Five seconds at most: a write that never arrives doesn't
/// stop the settings from being followed.
pub fn settled(rex: &RexSettings) -> bool {
    let mut carrying = CARRYING.lock().unwrap_or_else(|e| e.into_inner());
    let Some((ids, until)) = carrying.as_ref() else { return true };
    if ids.iter().all(|id| rex.hosts.iter().any(|h| &h.id == id)) || std::time::Instant::now() >= *until {
        *carrying = None;
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(id: &str, label: &str, endpoint: &str) -> RexHost {
        RexHost { id: id.into(), label: label.into(), endpoint: endpoint.into(), ..Default::default() }
    }

    /// Rex's file as the app wrote it before the panel: two hosts, one
    /// trusted with links and the clipboard, one with nothing; the done
    /// notice turned off. Carried over once, with nothing granted that
    /// wasn't; and never over what the desktop already says.
    #[test]
    fn what_rexs_own_file_held_is_carried_over_once() {
        let old: Old = serde_json::from_str(
            r#"{"saved_hosts":[{"id":"h1","label":"Build box","endpoint":"ssh://dev@build.example","auto_connect":true},{"id":"h2","label":"Staging","endpoint":"https://staging.example"}],
                "host_policies":{"h1":{"allow_open_url":true,"allow_localhost_autoforward":false,"allow_agent_screens":false,"allow_passwords_op":false,"allow_ssh_agent":true,"allow_remote_control":true,"allow_token_pool":true}},
                "notifications":{"working":{"tab_indicator":true,"desktop_notification":false,"badge":false,"sound":false,"volume":0.7},"blocked":{"tab_indicator":true,"desktop_notification":true,"badge":true,"sound":true,"volume":0.85},
                "error":{"tab_indicator":true,"desktop_notification":true,"badge":true,"sound":true,"volume":0.85},"done":{"tab_indicator":true,"desktop_notification":false,"badge":false,"sound":false,"volume":0.65}}}"#,
        )
        .unwrap();
        let mut rex = RexSettings::default();
        assert!(carry_over(&old, &mut rex));
        assert_eq!(rex.hosts.len(), 2);
        assert_eq!((rex.hosts[0].id.as_str(), rex.hosts[0].label.as_str(), rex.hosts[0].connect_at_start), ("h1", "Build box", true));
        assert_eq!(rex.hosts[0].grants, RexGrants { open_links: true, clipboard: true, ..Default::default() });
        assert!(!rex.hosts[1].grants.any() && !rex.hosts[1].connect_at_start);
        assert!(rex.notify_blocked && rex.notify_failed && !rex.notify_done);
        assert_eq!(rex.local, RexGrants::all());
        // Again: nothing more.
        assert!(!carry_over(&old, &mut rex));
        // What the desktop already says of a host stays.
        let mut theirs = RexSettings { hosts: vec![host("h1", "Mine now", "ssh://elsewhere")], notify_blocked: false, ..Default::default() };
        assert!(carry_over(&old, &mut theirs));
        assert_eq!((theirs.hosts[0].label.as_str(), theirs.hosts[0].endpoint.as_str()), ("Mine now", "ssh://elsewhere"));
        assert_eq!(theirs.hosts.len(), 2);
        assert!(!theirs.notify_blocked && theirs.notify_done, "notices already chosen there are left alone");
    }

    /// The first start with this build: what the file held is carried
    /// into the settings to follow, and marked done; and until those
    /// hosts have arrived in the desktop's settings, what it says isn't
    /// followed (it would say they are gone), but never for long.
    #[test]
    fn what_was_held_is_adopted_once_and_waited_for() {
        let mut s = Settings::read(Some(r#"{"saved_hosts":[{"id":"h1","label":"Build box","endpoint":"ssh://dev@build.example"}],"host_policies":{"h1":{"allow_open_url":true}}}"#));
        assert!(s.before_preferences.is_some() && !s.carried_to_preferences);
        let told = std::cell::Cell::new(0);
        let rex = adopt_telling(&mut s, RexSettings::default(), |old| told.set(old.saved_hosts.len()));
        assert_eq!((rex.hosts.len(), told.get()), (1, 1), "carried here, and the desktop told");
        assert!(rex.hosts[0].grants.open_links && !rex.hosts[0].grants.passwords);
        assert!(s.before_preferences.is_none() && s.carried_to_preferences);
        // The desktop's settings as they were before the write arrives: not yet.
        assert!(!settled(&RexSettings::default()));
        // With the host there: followed, and from then on whatever they say.
        assert!(settled(&rex));
        assert!(settled(&RexSettings::default()));
        // Again: nothing more to adopt.
        assert_eq!(adopt_telling(&mut s, RexSettings::default(), |_| panic!("told twice")), RexSettings::default());
        // A write that never arrives stops nothing for long.
        *CARRYING.lock().unwrap_or_else(|e| e.into_inner()) = Some((vec!["h1".into()], std::time::Instant::now()));
        assert!(settled(&RexSettings::default()));
    }

    /// Following: a host added, renamed, moved to another address and
    /// removed are each told as that; grants and notices follow; one
    /// still without an address isn't a host yet; the same again changes
    /// nothing.
    #[test]
    fn the_desktops_settings_are_followed() {
        let mut s = Settings::default();
        let mut rex = RexSettings { hosts: vec![host("a", "A", "ssh://a"), host("b", "", "ssh://b"), host("c", "Typing", " ")], ..Default::default() };
        rex.hosts[0].grants.passwords = true;
        rex.notify_done = false;
        let first = apply(&rex, &mut s);
        assert_eq!(first.added.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert!(first.any && first.removed.is_empty());
        assert_eq!(s.saved_hosts[1].label, "ssh://b", "a host with no name goes by its address");
        assert!(s.policy_for_host("a", false).allow_passwords_op && !s.policy_for_host("a", false).allow_open_url);
        assert!(!s.policy_for_host("b", false).allow_passwords_op && !s.policy_for_host("nobody", false).allow_open_url);
        assert!(s.policy_for_host("local", true).allow_remote_control);
        assert!(!s.notices.done && s.notices.blocked && s.notices.failed && !s.notices.minded_subagents);
        assert_eq!(apply(&rex, &mut s), Changes::default());
        rex.notify_minded_subagents = true;
        assert!(apply(&rex, &mut s).any);
        assert!(s.notices.minded_subagents);
        assert_eq!(apply(&rex, &mut s), Changes::default());
        rex.hosts[0].label = "Alpha".into();
        rex.hosts[1].endpoint = "ssh://b2".into();
        rex.hosts[2].endpoint = "ssh://c".into();
        let second = apply(&rex, &mut s);
        assert_eq!(second.renamed.iter().map(|h| h.label.as_str()).collect::<Vec<_>>(), ["Alpha"]);
        assert_eq!(second.moved.iter().map(|h| h.endpoint.as_str()).collect::<Vec<_>>(), ["ssh://b2"]);
        assert_eq!(second.added.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["c"]);
        rex.hosts.remove(0);
        rex.local.clipboard = false;
        let third = apply(&rex, &mut s);
        assert_eq!(third.removed, ["a"]);
        assert!(!s.policy_for_host("local", true).allow_remote_control);
        assert!(!s.policy_for_host("a", false).allow_passwords_op, "a host that is gone may do nothing");
    }
}
