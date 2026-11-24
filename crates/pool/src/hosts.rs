//! The desktop pool's side of the pools on Rex hosts
//! (`gateway::hosted`, `host`): the host's program put there over SSH
//! and run as the account's own service, the accounts carried to it
//! whenever they change here, a newer sign-in borrowed from it, and
//! how each host's pool stands, for the Token Pool's window and Rex's.
//!
//! The pool reaches a host itself, over SSH with the client Rex uses
//! (`olo_rex_client`: the person's keys, the host's key as Rex came to
//! know it), whether Rex is open or not: the accounts change when they
//! change, and a sign-in comes due when it comes due. Everything said
//! to the host's pool is one line to its own `olo-pool-host ctl`
//! there, and its answer.
//!
//! What it knows is kept in `pool-hosts.json` beside the vault (which
//! hosts have a pool, and what each was last carried): no secret is in
//! it.

use olo_sync::Mutex;
use std::{
    collections::{BTreeMap, HashMap},
    hash::{Hash, Hasher},
    sync::Arc,
    time::Duration,
};

use olo_rex_client::{SshSessionPool, SshTarget};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::gateway::{Carried, Hosting, Lender, Shared};
use crate::host::Ask;
use crate::model::{Credential, now};

/// The host's program, built for one kind of host.
#[derive(Clone, Copy, Debug)]
pub struct Build {
    /// What `uname -m` may say there.
    pub machines: &'static [&'static str],
    pub bytes: &'static [u8],
}

/// Where the program and its service live in the account's home.
const DIR: &str = ".local/share/olo/pool";
const UNIT: &str = "olo-pool.service";

/// How a host's pool stands, as the windows show it.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Stands {
    pub id: String,
    pub name: String,
    pub address: String,
    /// The person has a pool there (installed from here).
    pub wanted: bool,
    /// What is being done to it now ("Installing", "Carrying the
    /// accounts", "Removing").
    pub doing: Option<String>,
    /// Why the last thing asked of it didn't go.
    pub error: Option<String>,
    /// And trying again can't mend it (Olo has no pool for that kind
    /// of host).
    pub hopeless: bool,
    /// The accounts there are the accounts here.
    pub in_step: bool,
    /// Since when they aren't (seconds since the epoch).
    pub behind_since: Option<i64>,
    /// When they were last carried.
    pub carried_at: Option<i64>,
    /// How the host's pool said it stands, when it last answered
    /// (`Shared::hosted_status`: its accounts, requests and use).
    pub status: Option<Value>,
    /// When that was.
    pub heard_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Kept {
    #[serde(default)]
    wanted: bool,
    /// What was last carried and taken, by its digest; and when.
    #[serde(default)]
    carried: Option<u64>,
    #[serde(default)]
    carried_at: Option<i64>,
    #[serde(default)]
    behind_since: Option<i64>,
}

#[derive(Default)]
struct State {
    /// The hosts Rex knows that are reached over SSH: id, name, address.
    known: Vec<(String, String, String)>,
    kept: BTreeMap<String, Kept>,
    live: HashMap<String, Live>,
}

#[derive(Default, Clone)]
struct Live {
    doing: Option<String>,
    error: Option<String>,
    hopeless: bool,
    status: Option<Value>,
    heard_at: Option<i64>,
    /// When it is next tried by itself, while it is behind.
    retry_in: Option<u64>,
}

/// The pools on hosts, for one desktop pool.
pub struct Hosts {
    shared: Arc<Shared>,
    ssh: Arc<SshSessionPool>,
    builds: Vec<Build>,
    /// The carried program's name there (its build's own).
    build_id: String,
    state: Mutex<State>,
    changed: Box<dyn Fn() + Send + Sync>,
}

fn file() -> std::path::PathBuf {
    olo_xdg::config_dir().join("pool-hosts.json")
}

impl Hosts {
    /// `builds`: the host's program as the app carries it; `build_id`:
    /// its name (letters and digits). `changed` is called, from any
    /// thread, when a host stands differently.
    pub fn start(shared: Arc<Shared>, builds: Vec<Build>, build_id: &str, changed: impl Fn() + Send + Sync + 'static) -> Arc<Hosts> {
        let kept = std::fs::read(file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let hosts = Arc::new(Hosts {
            shared: shared.clone(),
            ssh: Arc::new(SshSessionPool::default()),
            builds,
            build_id: build_id.chars().filter(char::is_ascii_alphanumeric).collect(),
            state: Mutex::new(State { kept, ..State::default() }),
            changed: Box::new(changed),
        });
        // A newer sign-in for an account kept on a host: asked of it.
        let weak = Arc::downgrade(&hosts);
        let lender: Lender = Arc::new(move |host, id, at_least| {
            let weak = weak.clone();
            Box::pin(async move {
                let hosts = weak.upgrade().ok_or("the pool is closing")?;
                hosts.lend(&host, &id, at_least).await
            })
        });
        shared.set_lender(Some(lender));
        shared.set_hosting(Some(hosts.clone() as Arc<dyn Hosting>));
        // The accounts changed here: carried, a moment after.
        let weak = Arc::downgrade(&hosts);
        shared.on_accounts(move || {
            if let Some(hosts) = weak.upgrade() {
                hosts.carry_soon();
            }
        });
        hosts
    }

    /// The hosts Rex knows (Preferences ▸ Rex): those reached over SSH
    /// can have a pool.
    pub fn set_known(self: &Arc<Self>, hosts: Vec<(String, String, String)>) {
        let known: Vec<_> = hosts.into_iter().filter(|(_, _, address)| address.starts_with("ssh://")).collect();
        let changed = {
            let mut st = self.state.lock().unwrap();
            let changed = st.known != known;
            st.known = known;
            changed
        };
        if changed {
            self.told();
            self.carry_soon();
        }
    }

    /// Every host that can have a pool, and how its pool stands.
    pub fn list(&self) -> Vec<Stands> {
        let st = self.state.lock().unwrap();
        st.known
            .iter()
            .map(|(id, name, address)| {
                let kept = st.kept.get(id).cloned().unwrap_or_default();
                let live = st.live.get(id).cloned().unwrap_or_default();
                Stands {
                    id: id.clone(),
                    name: name.clone(),
                    address: address.trim_start_matches("ssh://").to_owned(),
                    wanted: kept.wanted,
                    doing: live.doing,
                    error: live.error,
                    hopeless: live.hopeless,
                    in_step: kept.wanted && kept.behind_since.is_none() && kept.carried.is_some(),
                    behind_since: kept.behind_since,
                    carried_at: kept.carried_at,
                    status: live.status,
                    heard_at: live.heard_at,
                }
            })
            .collect()
    }

    fn save(&self) {
        let kept = self.state.lock().unwrap().kept.clone();
        if let Ok(bytes) = serde_json::to_vec_pretty(&kept) {
            let _ = crate::store::write_private(&file(), &bytes);
        }
    }

    fn target(&self, id: &str) -> Result<SshTarget, String> {
        let address = self.state.lock().unwrap().known.iter().find(|(known, _, _)| known == id).map(|(_, _, address)| address.clone()).ok_or("Rex doesn’t know that host.")?;
        SshTarget::parse(&address).map_err(|e| e.to_string())
    }

    /// Whether the app carries the host's program at all (built
    /// without zig, it carries none: nothing can be installed).
    pub fn carried(&self) -> bool {
        self.builds.iter().any(|b| !b.bytes.is_empty())
    }

    fn name(&self, id: &str) -> String {
        self.state.lock().unwrap().known.iter().find(|(known, _, _)| known == id).map(|(_, name, _)| name.clone()).unwrap_or_else(|| id.to_owned())
    }

    fn live(&self, id: &str, change: impl FnOnce(&mut Live)) {
        change(self.state.lock().unwrap().live.entry(id.to_owned()).or_default());
        self.told();
    }

    /// How the hosts stand has changed: the app's window is told, and
    /// whoever waits on the gateway's `/pool/hosts?after=`.
    fn told(&self) {
        self.shared.hosts_changed();
        (self.changed)();
    }

    /// A command on the host, as the account, and what it wrote.
    async fn run(&self, target: &SshTarget, command: &str, input: Option<&[u8]>) -> Result<(bool, String), String> {
        let ran = match input {
            Some(input) => self.ssh.put(target, None, command, input).await,
            None => self.ssh.exec(target, None, command, &mut |_| {}).await,
        }
        .map_err(|e| e.to_string())?;
        Ok((ran.ok(), ran.output))
    }

    /// One thing asked of the host's pool, and its answer.
    async fn ask(&self, target: &SshTarget, ask: &Ask) -> Result<Value, String> {
        let line = serde_json::to_string(ask).map_err(|e| e.to_string())?;
        let (ok, said) = self.run(target, &format!("\"$HOME\"/{DIR}/olo-pool-host ctl"), Some(line.as_bytes())).await?;
        let last = said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default();
        if !ok {
            return Err(if last.is_empty() { "The Token Pool there didn’t answer.".to_owned() } else { last.trim_start_matches("olo-pool-host: ").to_owned() });
        }
        let answer: Value = serde_json::from_str(last).map_err(|_| "The Token Pool there answered something else.".to_owned())?;
        match answer["error"].as_str() {
            Some(why) => Err(why.to_owned()),
            None => Ok(answer),
        }
    }

    async fn lend(&self, host: &str, id: &str, at_least: i64) -> Result<Credential, String> {
        let target = self.target(host)?;
        let answer = self.ask(&target, &Ask::Lend { id: id.to_owned(), at_least }).await?;
        serde_json::from_value(answer["credential"].clone()).map_err(|_| "The Token Pool there lent nothing.".to_owned())
    }

    /// What decides whether a host has what is here: the accounts as
    /// they would be carried, without what a renewal changes for the
    /// host that keeps them.
    fn digest(carried: &Carried) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for a in &carried.accounts {
            (&a.id, &a.name, a.created, a.enabled, &a.kept_on, a.separate).hash(&mut h);
            let c = a.credential.as_ref();
            // (Handed over: its refresh token is the news. Lent: its
            // access token is.)
            c.map(|c| if a.kept_on.as_deref() == Some(carried.you.as_str()) { c.refresh.clone() } else { Some(c.access.clone()) }).hash(&mut h);
        }
        h.finish()
    }

    /// The accounts carried to host `id` now, and how it took them.
    async fn carry(&self, id: &str) -> Result<(), String> {
        let target = self.target(id)?;
        let carried = self.shared.carry_to(id, "this computer");
        let status = self.ask(&target, &Ask::Carry { carried }).await?;
        self.shared.handed_over(id, &status);
        // (As it stands after the handover: nothing more to tell it.)
        let digest = Self::digest(&self.shared.carry_to(id, "this computer"));
        {
            let mut st = self.state.lock().unwrap();
            let kept = st.kept.entry(id.to_owned()).or_default();
            (kept.carried, kept.carried_at, kept.behind_since) = (Some(digest), Some(now()), None);
            let live = st.live.entry(id.to_owned()).or_default();
            (live.status, live.heard_at, live.error, live.retry_in) = (Some(status), Some(now()), None, None);
        }
        self.save();
        Ok(())
    }

    /// The hosts whose pools don't have what is here.
    fn behind(&self) -> Vec<String> {
        let ids: Vec<String> = self.state.lock().unwrap().kept.iter().filter(|(_, k)| k.wanted).map(|(id, _)| id.clone()).collect();
        ids.into_iter()
            .filter(|id| {
                let st = self.state.lock().unwrap();
                st.known.iter().any(|(known, _, _)| known == id) && st.live.get(id).is_none_or(|l| l.doing.is_none())
            })
            .filter(|id| {
                // (Asked without marking anything: what would be carried, were it.)
                let now_is = Self::digest(&self.shared.would_carry(id, "this computer"));
                self.state.lock().unwrap().kept.get(id).is_some_and(|k| k.carried != Some(now_is))
            })
            .collect()
    }

    /// The accounts changed (or a host came to be known): every pool
    /// that doesn't have them is carried them, a moment after.
    pub fn carry_soon(self: &Arc<Self>) {
        let hosts = self.clone();
        self.shared.spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            for id in hosts.behind() {
                hosts.clone().carry_and_say(id, false).await;
            }
        });
    }

    /// Carried, and said: in step, or behind and why, to be tried again
    /// by itself (a minute, then two, up to ten) for as long as it is.
    async fn carry_and_say(self: Arc<Self>, id: String, asked: bool) {
        if self.state.lock().unwrap().live.get(&id).is_some_and(|l| l.doing.is_some()) {
            return;
        }
        self.live(&id, |l| l.doing = Some("Carrying the accounts".into()));
        let done = self.carry(&id).await;
        let again = {
            let mut st = self.state.lock().unwrap();
            let again = match &done {
                Ok(()) => None,
                Err(_) => {
                    let kept = st.kept.entry(id.clone()).or_default();
                    kept.behind_since.get_or_insert(now());
                    let live = st.live.entry(id.clone()).or_default();
                    // (Asked by the person: from the start again.)
                    let wait = if asked { 60 } else { live.retry_in.map_or(60, |w| (w * 2).min(600)) };
                    live.retry_in = Some(wait);
                    Some(wait)
                }
            };
            let live = st.live.entry(id.clone()).or_default();
            (live.doing, live.error) = (None, done.err());
            again
        };
        self.save();
        self.told();
        if let Some(wait) = again {
            let hosts = self.clone();
            self.shared.spawn(async move {
                tokio::time::sleep(Duration::from_secs(wait)).await;
                if hosts.behind().contains(&id) {
                    Box::pin(hosts.clone().carry_and_say(id, false)).await;
                }
            });
        }
    }

    /// The host's program put there and its service started (or
    /// started again, with the program the app carries now), then the
    /// accounts carried.
    async fn install(&self, id: &str) -> Result<(), String> {
        let target = self.target(id)?;
        let name = format!("olo-pool-host-{}", self.build_id);
        let (_, said) = self.run(&target, &format!("sh -c {}", word(&format!("uname -sm; [ -x \"$HOME\"/{DIR}/{name} ] && \"$HOME\"/{DIR}/{name} --version"))), None).await?;
        let mut lines = said.lines();
        let uname = lines.next().unwrap_or_default().to_owned();
        let there = lines.any(|l| l.trim() == format!("olo-pool-host {}", crate::host::VERSION));
        if !there {
            let mut kind = uname.split_whitespace();
            let (system, machine) = (kind.next().unwrap_or_default(), kind.next().unwrap_or_default());
            let build = self.builds.iter().find(|b| system == "Linux" && b.machines.contains(&machine) && !b.bytes.is_empty());
            let Some(build) = build else {
                self.live(id, |l| l.hopeless = true);
                return Err(format!("Olo has no Token Pool for {}.", if uname.is_empty() { "that host" } else { &uname }));
            };
            // (Whole, able to run there, and in its place in one move.)
            let takes = format!(
                "set -e; d=\"$HOME\"/{DIR}; mkdir -p \"$d\"; chmod 700 \"$d\"; t=\"$d/.new.$$\"; trap 'rm -f \"$t\"' EXIT; cat > \"$t\"; \
                 if [ \"$(wc -c < \"$t\" | tr -d ' ')\" != {len} ]; then echo 'it came short'; exit 3; fi; chmod 755 \"$t\"; \
                 if ! \"$t\" --version > /dev/null 2>&1; then echo 'it does not run here'; exit 4; fi; mv -f \"$t\" \"$d/{name}\"; \
                 find \"$d\" -type f -name 'olo-pool-host-*' ! -name {name} -exec rm -f {{}} + 2> /dev/null || true",
                len = build.bytes.len()
            );
            let (ok, said) = self.run(&target, &format!("sh -c {}", word(&takes)), Some(build.bytes)).await?;
            if !ok {
                return Err(format!("The Token Pool couldn’t be put there: {}", last(&said)));
            }
        }
        // Its service: the account's own unit where it has systemd
        // (kept running while it is signed out, where the host lets
        // it), else started now and again by Olo when found stopped.
        let service = format!(
            "set -e; d=\"$HOME\"/{DIR}; ln -sfn {name} \"$d/olo-pool-host\"; \
             echo '{{\"do\":\"stop\"}}' | \"$d/olo-pool-host\" ctl > /dev/null 2>&1 || true; \
             export XDG_RUNTIME_DIR=\"${{XDG_RUNTIME_DIR:-/run/user/$(id -u)}}\"; \
             if command -v systemctl > /dev/null 2>&1 && systemctl --user show-environment > /dev/null 2>&1; then \
               u=\"${{XDG_CONFIG_HOME:-$HOME/.config}}/systemd/user\"; mkdir -p \"$u\"; \
               printf '[Unit]\\nDescription=Olo Token Pool\\n\\n[Service]\\nExecStart=%s/olo-pool-host serve\\nRestart=on-failure\\nRestartSec=5\\n\\n[Install]\\nWantedBy=default.target\\n' \"$d\" > \"$u/{UNIT}\"; \
               systemctl --user daemon-reload; systemctl --user enable {UNIT} > /dev/null 2>&1 || true; systemctl --user restart {UNIT}; \
               loginctl enable-linger \"$(id -un)\" > /dev/null 2>&1 || true; \
             else (setsid nohup \"$d/olo-pool-host\" serve > /dev/null 2>&1 &); fi; \
             i=0; while [ $i -lt 40 ]; do if echo '{{\"do\":\"status\"}}' | \"$d/olo-pool-host\" ctl > /dev/null 2>&1; then echo running; exit 0; fi; i=$((i + 1)); sleep 0.25; done; \
             echo 'it did not start'; exit 5"
        );
        let (ok, said) = self.run(&target, &format!("sh -c {}", word(&service)), None).await?;
        if !ok {
            return Err(format!("The Token Pool’s service couldn’t be started there: {}", last(&said)));
        }
        self.state.lock().unwrap().kept.entry(id.to_owned()).or_default().wanted = true;
        self.save();
        self.carry(id).await
    }

    /// The host's pool taken away: every sign-in it keeps given back
    /// first (none is lost with it: where one can't be, nothing is
    /// removed), then its service, the clients there as they were, its
    /// files and its program.
    async fn remove(&self, id: &str) -> Result<(), String> {
        let target = self.target(id)?;
        for account in self.shared.kept_on(id) {
            let answer = self
                .ask(&target, &Ask::GiveBack { id: account.clone(), to: "this computer".into() })
                .await
                .map_err(|why| format!("A sign-in kept there couldn’t be taken back ({why}). Nothing was removed."))?;
            let credential: Credential = serde_json::from_value(answer["credential"].clone()).map_err(|_| "A sign-in kept there couldn’t be taken back. Nothing was removed.".to_owned())?;
            self.shared.took_back(&account, credential);
        }
        let gone = format!(
            "d=\"$HOME\"/{DIR}; export XDG_RUNTIME_DIR=\"${{XDG_RUNTIME_DIR:-/run/user/$(id -u)}}\"; \
             if command -v systemctl > /dev/null 2>&1; then systemctl --user disable --now {UNIT} > /dev/null 2>&1 || true; rm -f \"${{XDG_CONFIG_HOME:-$HOME/.config}}/systemd/user/{UNIT}\"; systemctl --user daemon-reload > /dev/null 2>&1 || true; fi; \
             echo '{{\"do\":\"stop\"}}' | \"$d/olo-pool-host\" ctl > /dev/null 2>&1 || true; \
             \"$d/olo-pool-host\" put-back 2>&1 || true; \"$d/olo-pool-host\" forget 2>&1 || true; rm -rf \"$d\"; echo removed"
        );
        let (ok, said) = self.run(&target, &format!("sh -c {}", word(&gone)), None).await?;
        if !ok {
            return Err(format!("It couldn’t be removed: {}", last(&said)));
        }
        {
            let mut st = self.state.lock().unwrap();
            st.kept.remove(id);
            st.live.remove(id);
        }
        self.save();
        Ok(())
    }

    /// How the host's pool stands now, asked of it.
    async fn look(&self, id: &str) -> Result<(), String> {
        let target = self.target(id)?;
        let status = self.ask(&target, &Ask::Status).await?;
        self.live(id, |l| (l.status, l.heard_at, l.error) = (Some(status), Some(now()), None));
        Ok(())
    }

    /// Every host that has a pool and nothing being done to it is asked
    /// how its pool stands, quietly (no "Asking…": for a window that
    /// shows them, now and then while it is open).
    pub fn refresh(self: &Arc<Self>) {
        let ids: Vec<String> = {
            let st = self.state.lock().unwrap();
            st.kept.iter().filter(|(id, k)| k.wanted && st.known.iter().any(|(known, _, _)| known == *id) && st.live.get(*id).is_none_or(|l| l.doing.is_none())).map(|(id, _)| id.clone()).collect()
        };
        for id in ids {
            let hosts = self.clone();
            self.shared.spawn(async move {
                if let Err(why) = hosts.look(&id).await {
                    hosts.live(&id, |l| l.error = Some(why));
                }
            });
        }
    }

    /// Does `what` to host `id` ("install", "remove", "sync", "look"),
    /// on the gateway's threads: how it goes shows in [`Self::list`].
    pub fn act(self: &Arc<Self>, id: &str, what: &str) -> Result<(), String> {
        self.target(id)?;
        let doing = match what {
            "install" => "Installing",
            "remove" => "Removing",
            "sync" => "Carrying the accounts",
            "look" => "Asking",
            _ => return Err(format!("There is no “{what}” for a host’s pool.")),
        };
        if self.state.lock().unwrap().live.get(id).is_some_and(|l| l.doing.is_some()) {
            return Err(format!("Something is being done to {} already.", self.name(id)));
        }
        if what == "sync" {
            let (hosts, id) = (self.clone(), id.to_owned());
            self.shared.spawn(async move { hosts.carry_and_say(id, true).await });
            return Ok(());
        }
        self.live(id, |l| (l.doing, l.error, l.hopeless) = (Some(doing.into()), None, false));
        let (hosts, id, what) = (self.clone(), id.to_owned(), what.to_owned());
        self.shared.spawn(async move {
            let done = match what.as_str() {
                "install" => hosts.install(&id).await,
                "remove" => hosts.remove(&id).await,
                _ => hosts.look(&id).await,
            };
            hosts.live(&id, |l| (l.doing, l.error) = (None, done.err()));
        });
        Ok(())
    }
}

impl Hosting for Hosts {
    fn list(&self) -> Value {
        json!({ "hosts": Hosts::list(self) })
    }

    fn act(self: Arc<Self>, id: &str, what: &str) -> Result<(), String> {
        Hosts::act(&self, id, what)
    }
}

fn word(s: &str) -> String {
    olo_rex_client::provision::shell_word(s)
}

fn last(said: &str) -> String {
    said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("the host said nothing").trim().to_owned()
}
