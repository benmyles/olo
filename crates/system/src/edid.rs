//! What a monitor says about itself in its EDID: maker, model name and
//! serial number (read from the kernel's copy in sysfs, for when the DRM
//! property route gives nothing, as on Apple Silicon's DCP), and its
//! colours and light (`color`): the base block's chromaticities, CTA-861's
//! HDR static metadata and colorimetry blocks, and DisplayID 2's display
//! parameters, where a laptop's panel keeps its native primaries and
//! luminance (the ThinkPad P1 Gen 9's OLED says nothing of HDR anywhere
//! else).

use olo_color::{Chromaticity, Primaries};
use olo_protocol::color::DisplayColor;

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
    let entry = dir.flatten().find(|e| e.file_name().to_string_lossy().split_once('-').is_some_and(|(card, c)| card.starts_with("card") && c == connector))?;
    parse(&std::fs::read(entry.path().join("edid")).ok()?)
}

/// The EDID of the monitor on a DRM connector ("DP-1"), as bytes.
pub fn bytes_for_connector(connector: &str) -> Option<Vec<u8>> {
    let dir = std::fs::read_dir("/sys/class/drm").ok()?;
    let entry = dir.flatten().find(|e| e.file_name().to_string_lossy().split_once('-').is_some_and(|(card, c)| card.starts_with("card") && c == connector))?;
    std::fs::read(entry.path().join("edid")).ok().filter(|b| b.len() >= 128)
}

/// What an EDID says of the display's colours and light.
pub fn color(edid: &[u8]) -> DisplayColor {
    let mut out = DisplayColor::default();
    if edid.len() < 128 || edid[..8] != [0, 255, 255, 255, 255, 255, 255, 0] {
        return out;
    }
    out.primaries = base_primaries(edid);
    if edid[23] != 0xff {
        out.gamma = Some((f64::from(edid[23]) + 100.0) / 100.0);
    }
    for block in edid[128..].chunks_exact(128).take(usize::from(edid[126])) {
        match block[0] {
            0x02 => cta(block, &mut out),
            0x70 => displayid(block, &mut out),
            _ => {}
        }
    }
    out
}

/// The base block's chromaticities (10 bits each: two low bits packed in
/// bytes 25–26, the high eight in 27–34).
fn base_primaries(e: &[u8]) -> Option<Primaries> {
    let lo = |byte: u8, shift: u8| u16::from((byte >> shift) & 3);
    let v = |hi: u8, low: u16| f64::from((u16::from(hi) << 2) | low) / 1024.0;
    let c = |x: f64, y: f64| Chromaticity { x, y };
    let p = Primaries {
        red: c(v(e[27], lo(e[25], 6)), v(e[28], lo(e[25], 4))),
        green: c(v(e[29], lo(e[25], 2)), v(e[30], lo(e[25], 0))),
        blue: c(v(e[31], lo(e[26], 6)), v(e[32], lo(e[26], 4))),
        white: c(v(e[33], lo(e[26], 2)), v(e[34], lo(e[26], 0))),
    };
    p.plausible().then_some(p)
}

/// A CTA-861 extension's colorimetry and HDR static metadata blocks.
fn cta(b: &[u8], out: &mut DisplayColor) {
    let end = usize::from(b[2]).min(127);
    let mut at = 4;
    while at < end {
        let (tag, len) = (b[at] >> 5, usize::from(b[at] & 0x1f));
        let Some(payload) = b.get(at + 1..at + 1 + len) else { break };
        if tag == 7 && !payload.is_empty() {
            match payload[0] {
                // Colorimetry: BT2020_RGB is bit 7.
                5 if payload.len() >= 2 => out.bt2020 |= payload[1] & 0x80 != 0,
                // HDR static metadata: EOTFs (bit 2 PQ, bit 3 HLG), then
                // the desired content's luminances, coded.
                6 if payload.len() >= 3 => {
                    out.pq |= payload[1] & 0x04 != 0;
                    out.hlg |= payload[1] & 0x08 != 0;
                    let nits = |cv: u8| 50.0 * 2f64.powf(f64::from(cv) / 32.0);
                    let max = payload.get(3).filter(|v| **v != 0).map(|v| nits(*v));
                    out.peak_nits = out.peak_nits.or(max);
                    out.full_nits = out.full_nits.or(payload.get(4).filter(|v| **v != 0).map(|v| nits(*v)));
                    if let (Some(max), Some(cv)) = (max, payload.get(5)) {
                        out.black_nits = out.black_nits.or(Some(max * (f64::from(*cv) / 255.0).powi(2) / 100.0).filter(|v| *v > 0.0));
                    }
                }
                _ => {}
            }
        }
        at += 1 + len;
    }
}

/// A DisplayID 2 extension's display parameters: the panel's native
/// primaries and white, its luminances, its gamma.
fn displayid(b: &[u8], out: &mut DisplayColor) {
    if b[1] >> 4 != 2 {
        return;
    }
    let end = (5 + usize::from(b[2])).min(127);
    let mut at = 5;
    while at + 3 <= end {
        let (tag, len) = (b[at], usize::from(b[at + 2]));
        let Some(p) = b.get(at + 3..at + 3 + len) else { break };
        if tag == 0x21 && len >= 29 {
            // Chromaticities: 12 bits each, x then y, 3 bytes a colour.
            let c = |i: usize| {
                let (b0, b1, b2) = (u16::from(p[9 + i * 3]), u16::from(p[10 + i * 3]), u16::from(p[11 + i * 3]));
                Chromaticity { x: f64::from(b0 | ((b1 & 0x0f) << 8)) / 4096.0, y: f64::from((b1 >> 4) | (b2 << 4)) / 4096.0 }
            };
            let native = Primaries { red: c(0), green: c(1), blue: c(2), white: c(3) };
            if native.plausible() {
                out.primaries = Some(native);
            }
            let nits = |i: usize| Some(half(u16::from_le_bytes([p[i], p[i + 1]]))).filter(|v| v.is_finite() && *v > 0.0);
            out.full_nits = nits(21).or(out.full_nits);
            out.peak_nits = nits(23).or(out.peak_nits);
            // (A black above 1 cd/m² is a panel's mistake: the
            // ThinkPad's OLED says 5.)
            out.black_nits = nits(25).filter(|v| *v < 1.0).or(out.black_nits);
            if p[28] != 0xff {
                out.gamma = Some((f64::from(p[28]) + 100.0) / 100.0);
            }
        }
        at += 3 + len;
    }
}

/// An IEEE 754 half-precision float.
fn half(h: u16) -> f64 {
    let (sign, exp, frac) = (if h & 0x8000 != 0 { -1.0 } else { 1.0 }, i32::from((h >> 10) & 0x1f), f64::from(h & 0x3ff));
    sign * match exp {
        0 => frac * 2f64.powi(-24),
        31 => f64::INFINITY,
        _ => (1.0 + frac / 1024.0) * 2f64.powi(exp - 15),
    }
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

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests/edid/{name}.bin", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn colours_of_the_thinkpads_oled() {
        // Samsung ATNA60KA02: DisplayID 2's native primaries (a little
        // wider than P3), 650 cd/m² full screen, 1600 peak, gamma 2.2;
        // no CTA block, so no HDR signal said there.
        let c = color(&fixture("thinkpad-p1-gen9-oled"));
        let p = c.primaries.unwrap();
        assert!((p.red.x - 0.6809).abs() < 1e-3 && (p.green.y - 0.7251).abs() < 1e-3 && (p.white.x - 0.3130).abs() < 1e-3, "{p:?}");
        assert_eq!((c.full_nits, c.peak_nits, c.black_nits, c.gamma), (Some(650.0), Some(1600.0), None, Some(2.2)));
        assert!(!c.pq && !c.bt2020);
        assert!(c.wide());
        assert_eq!(c.gamut().map(|g| g.0), Some("Display P3"));
    }

    #[test]
    fn colours_of_a_studio_display_and_a_dell() {
        // Apple's: P3, 600 cd/m² (CTA's static metadata, SDR only).
        let studio = color(&fixture("apple-studio-display"));
        assert!(studio.wide(), "{studio:?}");
        assert!(studio.peak_nits.is_some_and(|n| (n - 600.0).abs() < 10.0) && !studio.pq && studio.black_nits.is_none(), "{studio:?}");
        assert_eq!(studio.gamut().map(|g| g.0), Some("Display P3"));
        // Dell's: sRGB, nothing to convert.
        let dell = color(&fixture("dell-s2425hs"));
        assert!(!dell.wide(), "{dell:?}");
        assert!(dell.primaries().is_some());
    }

    #[test]
    fn half_floats() {
        assert_eq!(half(0x6114), 650.0);
        assert_eq!(half(0x6640), 1600.0);
        assert_eq!(half(0x3c00), 1.0);
        assert_eq!(half(0x0001), 2f64.powi(-24));
    }

    #[test]
    #[ignore]
    fn live() {
        for c in ["eDP-1", "DP-1", "DP-2", "DP-3", "HDMI-A-1"] {
            eprintln!("{c}: {:?}", for_connector(c));
        }
    }
}
