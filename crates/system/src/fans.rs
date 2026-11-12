//! The fans' keeper, as anyone may read it: whether this computer's
//! fans can be set at all (a ThinkPad's `/proc/acpi/ibm/fan`), the
//! profile chosen (`/etc/olo/fans`: `auto`, the firmware's own curve, or
//! `cool`, Olo's `olo-fans` running them ahead of the heat), whether
//! the service is installed, and what it says it's doing
//! (`/run/olo-fans/status`). Nothing here changes anything: the profile
//! is root's to write (Olo Doctor's helper: `olo-doctor --fans`).
//!
//! `OLO_FANS_ROOT=DIR` puts all of it under DIR (tests: a made-up
//! ThinkPad in a scratch folder, which Doctor's steps then write to as
//! the person, with nothing of systemd's asked).

use std::path::{Path, PathBuf};

/// Which curve the fans follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// The firmware's own.
    Auto,
    /// Olo's: raised early as anything warms, so nothing is throttled.
    Cool,
}

impl Profile {
    pub const ALL: [Profile; 2] = [Profile::Auto, Profile::Cool];

    /// As `/etc/olo/fans` and the command line say it.
    pub fn word(self) -> &'static str {
        match self {
            Profile::Auto => "auto",
            Profile::Cool => "cool",
        }
    }

    pub fn parse(word: &str) -> Option<Profile> {
        Profile::ALL.into_iter().find(|p| p.word() == word.trim())
    }

    /// As a menu names it.
    pub fn label(self) -> &'static str {
        match self {
            Profile::Auto => "Automatic",
            Profile::Cool => "Cool",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Profile::Auto => "The computer's own: quiet, and slow to speed the fans up. Under long, heavy work the processor is slowed down for the heat.",
            Profile::Cool => "The fans speed up ahead of the heat, so the processor isn't slowed down for it: cooler, and louder under work.",
        }
    }
}

/// Where the fans' things are: the system's own places, or a test's
/// tree (`OLO_FANS_ROOT`).
#[derive(Debug, Clone, PartialEq)]
pub struct Places {
    root: PathBuf,
    stand_in: bool,
}

impl Places {
    /// The system's, or `OLO_FANS_ROOT`'s.
    pub fn here() -> Places {
        match std::env::var_os("OLO_FANS_ROOT").filter(|r| !r.is_empty()) {
            Some(root) => Places::under(Path::new(&root)),
            None => Places { root: "/".into(), stand_in: false },
        }
    }

    /// A test's tree.
    pub fn under(root: &Path) -> Places {
        Places { root: root.to_owned(), stand_in: true }
    }

    /// A made-up tree: nothing of the real system's is touched or asked.
    pub fn stand_in(&self) -> bool {
        self.stand_in
    }

    fn at(&self, path: &str) -> PathBuf {
        self.root.join(path.trim_start_matches('/'))
    }

    /// thinkpad_acpi's fan file: read for the speed, written for a level.
    pub fn fan(&self) -> PathBuf {
        self.at("/proc/acpi/ibm/fan")
    }

    /// The profile chosen, one word.
    pub fn profile(&self) -> PathBuf {
        self.at("/etc/olo/fans")
    }

    /// What the service says it's doing.
    pub fn status(&self) -> PathBuf {
        self.at("/run/olo-fans/status")
    }

    /// The service's program, root's copy.
    pub fn program(&self) -> PathBuf {
        self.at("/usr/local/libexec/olo/olo-fans")
    }

    pub fn unit(&self) -> PathBuf {
        self.at("/etc/systemd/system/olo-fans.service")
    }

    /// Whether thinkpad_acpi takes levels at all (`Y`).
    pub fn fan_control(&self) -> PathBuf {
        self.at("/sys/module/thinkpad_acpi/parameters/fan_control")
    }

    /// Where it's told to, from the next start of the computer.
    pub fn modprobe(&self) -> PathBuf {
        self.at("/etc/modprobe.d/olo-fans.conf")
    }

    /// The service's own folder, there while it runs and only then:
    /// systemd makes it as the service starts and takes it away as it
    /// ends (the unit's `RuntimeDirectory`).
    pub fn running(&self) -> PathBuf {
        self.at("/run/olo-fans")
    }
}

/// What tells thinkpad_acpi to take levels at its next loading
/// (`Places::modprobe`). Olo Doctor's helper writes it, and so does
/// `olo-fans` itself when it finds the driver doesn't.
pub const CONF: &str = "# Olo's fan profile (Preferences > Power): the fans' level may be set.\noptions thinkpad_acpi fan_control=1\n";

/// Why a chosen Cool isn't in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Amiss {
    /// The driver doesn't take levels, and is told to at its next
    /// loading: in force from the computer's next start.
    NextStart,
    /// The driver doesn't take levels and nothing tells it to.
    Driver,
    /// The service isn't running.
    Stopped,
}

impl Amiss {
    /// For people, after "Cool profile ·" or "Not in force:".
    pub fn words(self) -> &'static str {
        match self {
            Amiss::NextStart => "the fans can't be set until the computer's next start",
            Amiss::Driver => "the fans' driver doesn't let them be set",
            Amiss::Stopped => "the fan service isn't running",
        }
    }
}

/// The fans' keeper as it is now.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    /// The profile in force: `/etc/olo/fans`' while the service is
    /// installed, else the firmware's own.
    pub profile: Profile,
    /// The service's program and unit are there.
    pub installed: bool,
    /// thinkpad_acpi takes levels (`fan_control=1`); if not, a chosen
    /// profile waits for the next start of the computer.
    pub control: bool,
    /// It is told to at its next loading ([`CONF`] is in its place).
    pub control_kept: bool,
    /// The service is running ([`Places::running`]). Not the same as
    /// its saying something: one from before 2026-11-12 says nothing
    /// until it first changes the level, and was read as not running
    /// through a cool morning.
    pub running: bool,
    /// The level the service holds ("auto", "3" to "7", "full-speed")
    /// and what asked for it; None: it has said nothing.
    pub held: Option<(String, String)>,
}

/// What there is; None where the fans can't be set (not a ThinkPad, or
/// thinkpad_acpi isn't loaded): nothing to choose.
pub fn look(places: &Places) -> Option<State> {
    if !places.fan().exists() {
        return None;
    }
    let installed = places.program().is_file() && places.unit().is_file();
    let chosen = std::fs::read_to_string(places.profile()).ok().and_then(|w| Profile::parse(&w));
    let control = std::fs::read_to_string(places.fan_control()).is_ok_and(|v| matches!(v.trim(), "Y" | "1"));
    let control_kept = std::fs::read_to_string(places.modprobe()).ok().as_deref() == Some(CONF);
    let running = places.running().is_dir();
    let held = if running { std::fs::read_to_string(places.status()).ok().and_then(|line| status(&line)) } else { None };
    Some(State { profile: if installed { chosen.unwrap_or(Profile::Auto) } else { Profile::Auto }, installed, control, control_kept, running, held })
}

/// `olo-fans`' status line ("level=6 why=coretemp Package id 0 74°"):
/// its level and why.
pub fn status(line: &str) -> Option<(String, String)> {
    let rest = line.trim().strip_prefix("level=")?;
    Some(match rest.split_once(" why=") {
        Some((level, why)) => (level.to_owned(), why.to_owned()),
        None => (rest.to_owned(), String::new()),
    })
}

impl State {
    /// Why Cool, chosen, isn't what the fans are doing (None: it is, or
    /// it isn't chosen).
    pub fn amiss(&self) -> Option<Amiss> {
        if self.profile != Profile::Cool {
            None
        } else if !self.control {
            Some(if self.control_kept { Amiss::NextStart } else { Amiss::Driver })
        } else if !self.running {
            Some(Amiss::Stopped)
        } else {
            None
        }
    }

    /// The level held, as the service says it; "auto" where it's
    /// running and has said nothing (an older one, until its first
    /// change), None where Cool isn't in force.
    pub fn level(&self) -> Option<(&str, &str)> {
        if self.profile != Profile::Cool || self.amiss().is_some() {
            return None;
        }
        Some(self.held.as_ref().map_or(("auto", ""), |(level, why)| (level.as_str(), why.as_str())))
    }

    /// What the fans are doing now, in a line.
    pub fn now(&self) -> String {
        if self.profile == Profile::Auto {
            return "Now: left to the computer.".to_owned();
        }
        match (self.amiss(), self.level()) {
            (Some(Amiss::NextStart), _) => "Starts with the computer's next start.".to_owned(),
            (Some(amiss), _) => format!("Not in force: {}. Olo Doctor repairs it.", amiss.words()),
            (None, Some(("auto", _))) | (None, None) => "Now: all is cool, the fans left to the computer.".to_owned(),
            (None, Some((level, why))) => {
                let level = if level == "full-speed" { "full speed".to_owned() } else { format!("level {level} of 7") };
                // (What the service couldn't do, said as it says it.)
                if why.is_empty() { format!("Now: {level}.") } else { format!("Now: {level}, for {why}.") }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str) -> (PathBuf, Places) {
        let root = PathBuf::from(format!("/var/tmp/olo-system-fans-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        (root.clone(), Places::under(&root))
    }

    fn put(path: PathBuf, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn nothing_to_choose_without_a_thinkpads_fan() {
        let (root, places) = tree("none");
        assert_eq!(look(&places), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_profile_is_the_firmwares_until_the_service_is_there() {
        let (root, p) = tree("look");
        put(p.fan(), "status:\t\tenabled\nspeed:\t\t2880\nlevel:\t\tauto\n");
        put(p.fan_control(), "Y\n");
        // (A word left by an install since removed means nothing.)
        put(p.profile(), "cool\n");
        let s = look(&p).unwrap();
        assert_eq!((s.profile, s.installed, s.control, s.held.clone()), (Profile::Auto, false, true, None));
        assert_eq!((s.now(), s.amiss(), s.level()), ("Now: left to the computer.".to_owned(), None, None));
        put(p.program(), "");
        put(p.unit(), "");
        // Chosen, and its service not running.
        let s = look(&p).unwrap();
        assert_eq!((s.amiss(), s.level()), (Some(Amiss::Stopped), None));
        assert_eq!(s.now(), "Not in force: the fan service isn't running. Olo Doctor repairs it.");
        // Running, and saying nothing yet (an older service until its
        // first change of level): the fans are the firmware's for now,
        // which isn't "not running".
        std::fs::create_dir_all(p.running()).unwrap();
        let s = look(&p).unwrap();
        assert_eq!((s.running, s.amiss(), s.level()), (true, None, Some(("auto", ""))));
        assert_eq!(s.now(), "Now: all is cool, the fans left to the computer.");
        put(p.status(), "level=4 why=19 W\n");
        let s = look(&p).unwrap();
        assert_eq!((s.profile, s.installed), (Profile::Cool, true));
        assert_eq!(s.now(), "Now: level 4 of 7, for 19 W.");
        put(p.status(), "level=auto why=\n");
        assert_eq!(look(&p).unwrap().now(), "Now: all is cool, the fans left to the computer.");
        put(p.status(), "level=full-speed why=SEN1 66°\n");
        assert_eq!(look(&p).unwrap().now(), "Now: full speed, for SEN1 66°.");
        // The driver doesn't take levels (loaded without fan_control=1):
        // whatever the service says, nothing is set. Not until someone
        // tells the driver; from the next start once it's told.
        put(p.fan_control(), "N\n");
        let s = look(&p).unwrap();
        assert_eq!((s.amiss(), s.level()), (Some(Amiss::Driver), None));
        assert_eq!(s.now(), "Not in force: the fans' driver doesn't let them be set. Olo Doctor repairs it.");
        put(p.modprobe(), CONF);
        let s = look(&p).unwrap();
        assert_eq!((s.amiss(), s.control_kept), (Some(Amiss::NextStart), true));
        assert_eq!(s.now(), "Starts with the computer's next start.");
        put(p.profile(), "auto\n");
        assert_eq!(look(&p).unwrap().profile, Profile::Auto);
        let _ = std::fs::remove_dir_all(root);
    }

    /// What a look costs (Activity Monitor takes one with each sample,
    /// Preferences every five seconds): `cargo test --release -p
    /// olo-system look_costs -- --ignored --nocapture`, on a ThinkPad.
    #[test]
    #[ignore]
    fn look_costs() {
        let places = Places::here();
        let n = 20_000;
        let began = std::time::Instant::now();
        let mut found = 0;
        for _ in 0..n {
            found += look(&places).is_some() as u32;
        }
        let each = began.elapsed().as_secs_f64() * 1e6 / n as f64;
        let began = std::time::Instant::now();
        for _ in 0..n {
            found += std::fs::read_to_string(places.fan_control()).is_ok() as u32;
        }
        let one = began.elapsed().as_secs_f64() * 1e6 / n as f64;
        println!("look: {each:.1} µs; the driver's fan_control read alone: {one:.1} µs ({found})");
    }

    #[test]
    fn words() {
        assert_eq!(Profile::parse("cool\n"), Some(Profile::Cool));
        assert_eq!(Profile::parse("full"), None);
        assert_eq!(status("level=6 why=coretemp Package id 0 74°"), Some(("6".into(), "coretemp Package id 0 74°".into())));
        assert_eq!(status("nothing"), None);
    }
}
