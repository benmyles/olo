//! What a monitor says about itself in its EDID: maker, model name and
//! serial number. Read from the kernel's copy in sysfs, for when the DRM
//! property route gives nothing (as on Apple Silicon's DCP).

/// A monitor's identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edid {
    /// The three-letter PNP id ("APP", "DEL").
    pub maker: String,
    /// The display name descriptor, if it has one.
    pub model: Option<String>,
    /// The serial number descriptor, else the numeric serial if set.
    pub serial: Option<String>,
}

/// Parse the base block of an EDID.
pub fn parse(edid: &[u8]) -> Option<Edid> {
    if edid.len() < 128 || edid[..8] != [0, 255, 255, 255, 255, 255, 255, 0] {
        return None;
    }
    let id = u16::from_be_bytes([edid[8], edid[9]]);
    let maker: String = [10, 5, 0].iter().map(|s| (((id >> s) & 31) as u8 + b'@') as char).collect();
    if !maker.chars().all(|c| c.is_ascii_uppercase()) {
        return None;
    }
    let mut model = None;
    let mut serial = None;
    // Four 18-byte descriptors; display descriptors start 0, 0, 0, tag.
    for d in edid[54..126].chunks(18) {
        if d[..3] != [0, 0, 0] {
            continue;
        }
        let text = || {
            let s: String = d[5..18].iter().take_while(|&&b| b != b'\n').map(|&b| b as char).collect();
            let s = s.trim().to_owned();
            (!s.is_empty() && s.chars().all(|c| c.is_ascii_graphic() || c == ' ')).then_some(s)
        };
        match d[3] {
            0xfc => model = text(),
            0xff => serial = text(),
            _ => {}
        }
    }
    let number = u32::from_le_bytes([edid[12], edid[13], edid[14], edid[15]]);
    if serial.is_none() && number != 0 {
        serial = Some(number.to_string());
    }
    Some(Edid { maker, model, serial })
}

/// The EDID of the monitor on a DRM connector ("DP-1"), from sysfs.
pub fn for_connector(connector: &str) -> Option<Edid> {
    let dir = std::fs::read_dir("/sys/class/drm").ok()?;
    let entry = dir
        .flatten()
        .find(|e| e.file_name().to_string_lossy().split_once('-').is_some_and(|(card, c)| card.starts_with("card") && c == connector))?;
    parse(&std::fs::read(entry.path().join("edid")).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up EDID: maker APP, name "StudioDisplay", serial "ABC123".
    fn sample(name: &str, serial: Option<&str>, number: u32) -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 0]);
        // "APP": A=1, P=16, P=16.
        let id: u16 = (1 << 10) | (16 << 5) | 16;
        e[8..10].copy_from_slice(&id.to_be_bytes());
        e[12..16].copy_from_slice(&number.to_le_bytes());
        let mut put = |at: usize, tag: u8, text: &str| {
            e[at..at + 5].copy_from_slice(&[0, 0, 0, tag, 0]);
            let mut t: Vec<u8> = text.bytes().collect();
            t.push(b'\n');
            t.resize(13, b' ');
            e[at + 5..at + 18].copy_from_slice(&t);
        };
        put(72, 0xfc, name);
        if let Some(s) = serial {
            put(90, 0xff, s);
        }
        e
    }

    #[test]
    fn reads_name_and_serial() {
        let e = parse(&sample("StudioDisplay", Some("ABC123"), 0)).unwrap();
        assert_eq!(e, Edid { maker: "APP".into(), model: Some("StudioDisplay".into()), serial: Some("ABC123".into()) });
        // No serial descriptor: the number.
        assert_eq!(parse(&sample("X", None, 4242)).unwrap().serial.as_deref(), Some("4242"));
        assert_eq!(parse(&sample("X", None, 0)).unwrap().serial, None);
        assert_eq!(parse(&[0; 128]), None);
    }

    #[test]
    #[ignore]
    fn live() {
        for c in ["eDP-1", "DP-1", "DP-2", "DP-3", "HDMI-A-1"] {
            eprintln!("{c}: {:?}", for_connector(c));
        }
    }
}
