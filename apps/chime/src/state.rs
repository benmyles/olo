//! Whether the person muted the speakers, as the computer starts: no
//! sound server runs yet, so read from what theirs kept when it last ran
//! (WirePlumber's state, `~/.local/state/wireplumber/default-routes`: a
//! line for each card's route with its volume and whether it's muted).
//! On a card whose speakers have no volume of their own (a SoundWire
//! laptop's: the sound server applies it), that file is the only place
//! the person's mute is; the card itself plays at full.
//!
//! ```text
//! alsa_card.pci-0000_00_1f.3-platform-sof_sdw:output:\oOut\c\sSpeaker={"channelVolumes":[0.08, 0.08], "mute":false, …}
//! ```

use std::path::{Path, PathBuf};

/// The people's homes (`root`: "/"): the accounts from 1000 up in
/// /etc/passwd.
pub fn homes(root: &Path) -> Vec<PathBuf> {
    let passwd = std::fs::read_to_string(root.join("etc/passwd")).unwrap_or_default();
    passwd
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split(':').collect();
            let (uid, home) = (f.get(2)?.parse::<u32>().ok()?, f.get(5)?);
            (1000..60000).contains(&uid).then(|| root.join(home.trim_start_matches('/')))
        })
        .collect()
}

/// The sound server's name for card `index`, as it makes it of udev's
/// (`alsa_card.` and the card's id or, with none, its path; what isn't a
/// letter, digit, `_`, `-` or `.` an underscore).
pub fn card_name(root: &Path, index: u32) -> Option<String> {
    let data = std::fs::read_to_string(root.join(format!("run/udev/data/+sound:card{index}"))).ok()?;
    let value = |key: &str| data.lines().find_map(|l| l.strip_prefix("E:")?.strip_prefix(key)?.strip_prefix('=')).filter(|v| !v.is_empty());
    let id = value("ID_ID").or_else(|| value("ID_PATH"))?;
    Some(format!("alsa_card.{}", id.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') { c } else { '_' }).collect::<String>()))
}

/// A key as it was before the state file escaped it.
fn unescaped(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut chars = key.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('e') => out.push('='),
            Some('o') => out.push('['),
            Some('c') => out.push(']'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Whether `card`'s speakers are muted, as the state file `text` has
/// them (any card's, `card` unknown): none if it has no such route.
pub fn speakers_muted(text: &str, card: Option<&str>) -> Option<bool> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        let (of, route) = key.split_once(":output:")?;
        if !of.starts_with("alsa_card.") || card.is_some_and(|c| c != of) || !unescaped(route).to_lowercase().contains("speaker") {
            return None;
        }
        serde_json::from_str::<serde_json::Value>(value).ok()?.get("mute")?.as_bool()
    })
}

/// Whether the speakers of card `index` are muted, as the person who
/// used the sound last left them (the state changed last, of those that
/// know the card's speakers). No state: not muted.
pub fn muted(root: &Path, index: u32) -> bool {
    let card = card_name(root, index);
    homes(root)
        .iter()
        .filter_map(|home| {
            let path = home.join(".local/state/wireplumber/default-routes");
            let muted = speakers_muted(&std::fs::read_to_string(&path).ok()?, card.as_deref())?;
            Some((std::fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), muted))
        })
        .max_by_key(|(when, _)| *when)
        .is_some_and(|(_, muted)| muted)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATE: &str = r#"[default-routes]
alsa_card.pci-0000_00_1f.3-platform-sof_sdw:profile:HiFi\s(HDMI1,\sHDMI2,\sHDMI3,\sHeadphones,\sMic,\sSpeaker)=["[Out] Speaker"]
alsa_card.pci-0000_01_00.1:output:hdmi-output-0={"channelVolumes":[1.0, 1.0], "mute":true, "channelMap":["FL", "FR"], "latencyOffsetNsec":0}
alsa_card.usb-Some_Speaker-00:output:analog-output-speaker={"channelVolumes":[0.4, 0.4], "mute":true, "channelMap":["FL", "FR"], "latencyOffsetNsec":0}
alsa_card.pci-0000_00_1f.3-platform-sof_sdw:output:\oOut\c\sHeadphones={"channelVolumes":[0.2, 0.2], "mute":true, "channelMap":["FL", "FR"], "latencyOffsetNsec":0}
alsa_card.pci-0000_00_1f.3-platform-sof_sdw:output:\oOut\c\sSpeaker={"channelVolumes":[0.083740, 0.083740], "mute":false, "channelMap":["FL", "FR"], "latencyOffsetNsec":0}
bluez_card.38_C4_3A_3E_A0_B0:output:headphone-output={"channelVolumes":[0.1, 0.1], "mute":true}
"#;

    fn write(root: &Path, path: &str, text: &str) {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// The card's own speakers' line, among other cards', other routes'
    /// and a profile's.
    #[test]
    fn the_speakers_line() {
        let card = "alsa_card.pci-0000_00_1f.3-platform-sof_sdw";
        assert_eq!(speakers_muted(STATE, Some(card)), Some(false));
        assert_eq!(speakers_muted(&STATE.replace(r#"0.083740], "mute":false"#, r#"0.083740], "mute":true"#), Some(card)), Some(true));
        assert_eq!(speakers_muted(STATE, Some("alsa_card.usb-Some_Speaker-00")), Some(true), "an older card's own route name");
        assert_eq!(speakers_muted(STATE, Some("alsa_card.pci-0000_01_00.1")), None, "HDMI: no speakers");
        assert_eq!(speakers_muted("", Some(card)), None);
        assert_eq!(unescaped(r"\oOut\c\sSpeaker"), "[Out] Speaker");
    }

    /// The card's name from udev's, and the mute of whoever used the
    /// sound last.
    #[test]
    fn muted_as_last_left() {
        let root = PathBuf::from(format!("/var/tmp/olo-chime-test-{}-state", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "etc/passwd", "root:x:0:0:root:/root:/bin/bash\nben:x:1000:1000::/home/ben:/bin/brush\nsam:x:1001:1001::/home/sam:/bin/bash\n");
        write(&root, "run/udev/data/+sound:card1", "E:ID_PATH=pci-0000:00:1f.3-platform-sof_sdw\nE:ID_BUS=pci\n");
        write(&root, "run/udev/data/+sound:card2", "E:ID_PATH=pci-0000:ac:00.0-usb-0:1.4:1.2\nE:ID_ID=usb-Some_Speaker-00\n");
        assert_eq!(card_name(&root, 1).as_deref(), Some("alsa_card.pci-0000_00_1f.3-platform-sof_sdw"));
        assert_eq!(card_name(&root, 2).as_deref(), Some("alsa_card.usb-Some_Speaker-00"));
        assert_eq!(card_name(&root, 7), None);
        assert!(!muted(&root, 1), "no state: not muted");
        let routes = ".local/state/wireplumber/default-routes";
        write(&root, &format!("home/ben/{routes}"), STATE);
        assert!(!muted(&root, 1));
        // Sam used it since, and muted the speakers.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&root, &format!("home/sam/{routes}"), &STATE.replace(r#"0.083740], "mute":false"#, r#"0.083740], "mute":true"#));
        assert!(muted(&root, 1));
        // Then Ben again, not muted.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&root, &format!("home/ben/{routes}"), STATE);
        assert!(!muted(&root, 1));
        let _ = std::fs::remove_dir_all(&root);
    }
}
