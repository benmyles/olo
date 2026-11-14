//! The sound cards, as /proc/asound says, and which is the computer's
//! own: what olo-chime plays the chime to as the computer starts, and
//! what Olo Doctor asks before offering it.

use std::path::Path;

/// A sound card.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub index: u32,
    /// Its id ("sofsoundwire").
    pub id: String,
    pub usb: bool,
    /// Its playback PCMs: device and id ("Speaker (*)", "HDMI 0").
    pub playback: Vec<(u32, String)>,
}

impl Card {
    /// Its speakers' PCM: one named so, else its first that isn't HDMI,
    /// DisplayPort or a deep buffer.
    pub fn speakers(&self) -> Option<(u32, String)> {
        let analog = |id: &str| !(id.contains("HDMI") || id.contains("DP") || id.contains("DisplayPort") || id.starts_with("Deepbuffer"));
        self.playback.iter().find(|(_, id)| id.starts_with("Speaker")).or_else(|| self.playback.iter().find(|(_, id)| analog(id))).cloned()
    }
}

/// The sound cards under `root` (its proc/asound).
pub fn cards(root: &Path) -> Vec<Card> {
    let asound = root.join("proc/asound");
    let list = std::fs::read_to_string(asound.join("cards")).unwrap_or_default();
    list.lines()
        .filter_map(|l| {
            // " 1 [sofsoundwire   ]: sof-soundwire - sof-soundwire"
            let (index, rest) = l.trim_start().split_once(' ')?;
            let index: u32 = index.parse().ok()?;
            let id = rest.trim_start().strip_prefix('[')?.split(']').next()?.trim().to_owned();
            let dir = asound.join(format!("card{index}"));
            let mut playback: Vec<(u32, String)> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let device: u32 = name.strip_prefix("pcm")?.strip_suffix('p')?.parse().ok()?;
                    let info = std::fs::read_to_string(e.path().join("info")).ok()?;
                    let id = info.lines().find_map(|l| l.strip_prefix("id: "))?.trim().to_owned();
                    Some((device, id))
                })
                .collect();
            playback.sort();
            Some(Card { index, id, usb: dir.join("usbid").exists(), playback })
        })
        .collect()
}

/// The computer's own sound card: not USB, with speakers (a playback PCM
/// not HDMI's or DisplayPort's).
pub fn built_in(cards: &[Card]) -> Option<Card> {
    cards.iter().find(|c| !c.usb && c.speakers().is_some()).cloned()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// The ThinkPad P1's cards, as its /proc says: NVIDIA's HDMI only,
    /// the built-in SoundWire one, the Studio Display's over USB.
    #[test]
    fn the_built_in_card_and_its_speakers() {
        let root = PathBuf::from(format!("/var/tmp/olo-chime-cards-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(
            &root,
            "proc/asound/cards",
            " 0 [NVidia         ]: HDA-Intel - HDA NVidia\n                      HDA NVidia at 0xa8000000 irq 17\n 1 [sofsoundwire   ]: sof-soundwire - sof-soundwire\n                      LENOVO-21UECTO1WW-ThinkPadP1Gen9\n 2 [Display        ]: USB-Audio - Studio Display\n                      Apple Inc. Studio Display\n",
        );
        write(&root, "proc/asound/card0/pcm3p/info", "card: 0\ndevice: 3\nid: HDMI 0\n");
        write(&root, "proc/asound/card1/pcm0p/info", "card: 1\ndevice: 0\nid: Jack Out (*)\n");
        write(&root, "proc/asound/card1/pcm2p/info", "card: 1\ndevice: 2\nid: Speaker (*)\n");
        write(&root, "proc/asound/card1/pcm35p/info", "card: 1\ndevice: 35\nid: Deepbuffer Speaker (*)\n");
        write(&root, "proc/asound/card1/pcm1c/info", "card: 1\ndevice: 1\nid: Jack In (*)\n");
        write(&root, "proc/asound/card2/pcm0p/info", "card: 2\ndevice: 0\nid: USB Audio\n");
        write(&root, "proc/asound/card2/usbid", "05ac:1114\n");
        let all = cards(&root);
        assert_eq!(all.len(), 3);
        let card = built_in(&all).expect("the built-in card");
        assert_eq!((card.index, card.id.as_str()), (1, "sofsoundwire"));
        assert_eq!(card.speakers(), Some((2, "Speaker (*)".into())));
        // A laptop's HDA card: its analog PCM.
        let hda = Card { index: 0, id: "PCH".into(), usb: false, playback: vec![(0, "ALC3287 Analog".into()), (3, "HDMI 0".into())] };
        assert_eq!(hda.speakers(), Some((0, "ALC3287 Analog".into())));
        // Only a USB card and HDMI: none.
        assert_eq!(built_in(&[all[0].clone(), all[2].clone()]), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
